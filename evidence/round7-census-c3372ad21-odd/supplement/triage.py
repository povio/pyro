"""Reconstruct all former LP pass losses from raw three-engine results."""
import argparse,collections,gzip,hashlib,importlib.util,json,re
from pathlib import Path
def j(p):return json.loads(p.read_bytes())
def rows(p):
    with gzip.open(p,'rb') as f:yield from map(json.loads,f)
def sha(p):
    with p.open('rb') as f:return hashlib.file_digest(f,'sha256').hexdigest()
def counts(xs):return dict(collections.Counter(xs))
def save(p,o):p.write_text(json.dumps(o,indent=1,ensure_ascii=True)+'\n')
def require(v,why):
    if not v:raise ValueError(why)

def run(args):
    final=args.final;out=args.out;out.mkdir(exist_ok=True,parents=True)
    sums={n:d for d,n in [l.split('  ',1) for l in (final/'SHA256SUMS').read_text().splitlines()]}
    def checked(n):
        f=final/n;require(sha(f)==sums[n],n);return f
    meta={t:j(checked(t+'-provenance.json')) for t in ['current','previous','chrome']}
    norm=args.chrome/'export-final/canonical_ports.py'
    require(sha(norm)=='48102949ce8412b426c763bd5bfcf194a425ce3123624b424abf2e0bcaf61a5b','canonicalizer')
    sp=importlib.util.spec_from_file_location('norm',norm);canon=importlib.util.module_from_spec(sp);sp.loader.exec_module(canon)
    summary=j(checked('summary.json'));loss=list(rows(checked('losses.jsonl.gz')))
    subset={tuple(x[:3]) for x in rows(checked('named.jsonl.gz')) if x[5] and x[5][0]=='Pass' and not (x[3] and x[3][0]=='Pass')}
    require(len(loss)==len(subset)==summary['counts']['currentPrevious']['secondOnly'] and subset=={tuple(x[:3]) for x in loss},'loss set')
    urls={k[0] for k in subset};pages={x[0]:x for x in rows(checked('pages.jsonl.gz')) if x[0] in urls};raw={};hashes={}
    even=j(args.even/'local-supplement/local-loss-triage.json')
    reviewed={tuple([r['url'],r['canonicalName'],r['occurrence']]):r for r in even['records']}
    for u,row in pages.items():
        for tag,index,field in [('current',0,'receiptSHA256'),('chrome',1,'rawSHA256'),('previous',2,'receiptSHA256')]:
            key=row[4][index].removeprefix('chrome/');proof=meta[tag][key];k=tag,key
            if k in hashes:continue
            root=(args.odd if proof['index']%2 else args.even) if tag=='current' else args.previous if tag=='previous' else args.chrome
            f=root/'raw'/(key+'.json');require(sha(f)==proof[field],f);hashes[k]=proof[field]
            vport=root/'verified-ports'/('chrome' if tag=='chrome' else '')/(key+'.json')
            if tag in ('current','chrome'):
                ports=j(vport)['proof']['ports']
            else:ports=proof['ports']
            mapping={}
            for role,vals in ports.items():
                for p,default in zip(vals,canon.DEFAULTS.get(role,[])):mapping['https' if role=='h2' else role.split('-')[0],p]=default
            for r in j(f)['results']:
                if r['url'] in urls:raw[tag,r['url']]=r,key,mapping
    data=[]
    for u,n,i,h,c,b,pageStatus,harness,msg,key in loss:
        ref=reviewed.get((u,n,i));states={};related=[]
        for tag,cell,pi in [('current',h,1),('chrome',c,2),('previous',b,3)]:
            rec,k,mapping=raw[tag,u];pg=pages[u][pi]
            require(pg[:2]==[rec['status'],rec.get('harnessStatus')],(tag,u,'lifecycle'))
            candidates=[t for t in rec['subtests'] if canon.canonical_name(t['name'],mapping)==n]
            t=candidates[i-1] if len(candidates)>=i else None
            require(([t['status'],t['name']] if t else None)==cell,(tag,u,n,i))
            states[tag]=dict(receiptKey=k,receiptSHA256=hashes[tag,k],lifecycle=pg[0],harnessStatus=pg[1],strictPage=pg[2],passingNames=pg[3],observedNames=pg[4],pageDiagnostic=rec.get('error'),harnessMessage=rec.get('harnessMessage'),subtestStatus=t['status'] if t else None,originalReportedName=t['name'] if t else None,subtestDiagnostic=t.get('msg') if t else None)
        require(states['current']['subtestDiagnostic']==msg and states['previous']['subtestStatus']=='Pass','raw diagnostic')
        ordinal=meta['current'][key]['index'];require(meta['current'][key]['machine']['machine']=='local','executor')
        if ref:
            sig,family,explanation=ref['signature'],ref['behaviorFamily'],ref['diagnosticInterpretation']
            for tag,priorTag in [('current','current'),('chrome','signed'),('previous','prior')]:
                also=ref['engines'][priorTag].get('samePageRelatedObservations')
                if also:states[tag]['samePageRelatedObservations']=also
        else:sig,family,explanation=classify(u,n,h,msg)
        if sig=='generated-signature-name':
            signature_pattern=r'signature=:[^:]*:'
            trim=lambda x:re.sub(signature_pattern,'signature=<generated>',x)
            for tag in ('current','chrome'):
                original=raw[tag,u][0]
                matches=[dict(originalReportedName=v['name'],subtestStatus=v['status'],subtestDiagnostic=v.get('msg')) for v in original['subtests'] if trim(v['name'])==trim(n)]
                if matches:states[tag]['samePageRelatedObservations']=matches
        data.append(dict(url=u,canonicalName=n,occurrence=i,observationClass=('missing exact name' if h is None else 'observed assertion failure' if h[0]=='Fail' else 'subtest '+h[0]),originalOrdinal=ordinal,originalAllocation='studio' if ordinal%2 else 'local',actualExecutor='local',domain=u.split('/')[0],signature=sig,behaviorFamily=family,diagnosticInterpretation=explanation,implementationCause='Unverified. These measurements establish observed behavior, without attributing a commit.',engines=states))
    groups=collections.defaultdict(list)
    for r in data:groups[r['signature']].append(r)
    def stat(a):
        return dict(n=len(a),urls=len({t['url'] for t in a}),currentStatus=counts(t['engines']['current']['subtestStatus'] or 'Missing' for t in a),signedStatus=counts(t['engines']['chrome']['subtestStatus'] or 'Missing' for t in a),pageTransitions=counts(str(t['engines']['previous']['lifecycle'])+'/'+str(t['engines']['previous']['harnessStatus'])+' -> '+str(t['engines']['current']['lifecycle'])+'/'+str(t['engines']['current']['harnessStatus']) for t in a))
    grouped=[dict(signature=k,family=a[0]['behaviorFamily'],interpretation=a[0]['diagnosticInterpretation'],**stat(a)) for k,a in sorted(groups.items(),key=lambda b:(-len(b[1]),b[0]))]
    payload=dict(head=summary['head'],wpt=summary['wpt'],planSHA256=summary['planSHA256'],snapshot=summary['snapshot'],summary=stat(data),verifiedRawReceipts=len(hashes),groups=grouped,records=data)
    save(out/'loss-triage.json',payload)
    md=['# Prior passing occurrences lost','','These rows compare current LP with prior LP on the same URLs. The JSON includes the signed Chrome status, exact occurrence key, original shard ordinal, raw diagnostics and receipt digests for each observation. Both ordinal assignments ran on the local machine.','','| Diagnostic group | Occurrences | URLs | Observed behavior |','|---|---:|---:|---|']
    total=payload['summary']; head=total['currentStatus'];signed=total['signedStatus']
    md[3:3]=['There are '+str(total['n'])+' lost canonical occurrences across '+str(total['urls'])+' URLs. Current LP reported '+str(head.get('Fail',0))+' named failures, '+str(head.get('Timeout',0))+' named timeouts, '+str(head.get('Not Run',0))+' reported Not Run and '+str(head.get('Missing',0))+' absent names. Signed Chrome passed '+str(signed.get('Pass',0))+', failed '+str(signed.get('Fail',0))+', and had no exact match for '+str(signed.get('Missing',0))+'.', '']
    for r in grouped:md.append(f"| {r['signature']} | {r['n']} | {r['urls']} | {r['interpretation']} |")
    md+=['','Missing exact names, subtest timeouts and observed failed assertions retain distinct statuses. Groups describe the recorded symptoms. The underlying implementation cause is unverified.','']
    (out/'loss-triage.md').write_text('\n'.join(md))
    print(json.dumps(payload['summary']))

def classify(u,n,h,msg):
    msg=msg or ''
    if not h and 'subresource-integrity/signatures/' in u and 'signature=:' in n:
        return 'generated-signature-name','Recorded name changes','The prior original name contains a generated signature string absent under the exact canonical key. Related same-page observations are listed separately; they do not replace its score.'
    if not h and 'fetch/metadata/generated/window-location' in u:
        return 'window-location-missing','Availability','Two previously passing sec-fetch-user operations were not emitted on a page with a current timeout.'
    if not h:return 'missing-exact-name' ,'Availability','Prior occurrence absent from the current named ledger.'
    if h[0]=='Not Run':return 'reported-not-run','Availability','The runner emitted the name with Not Run status; it did not produce an assertion result.'
    if h[0]=='Timeout' and 'open_fires_resize' in u:return 'popup-resize-timeout','Availability','A popup resize-event test timed out before it reached an assertion.'
    if h[0]=='Timeout':return 'named-timeout','Availability','The current runner recorded a subtest timeout.'
    if 'Popup name is cleared?' in msg:return 'popup-name-not-cleared','Auxiliary browsing contexts','COOP popup retained its name where the assertion expected it cleared.'
    if 'expected 204 but got 200' in msg or 'expected 304 but got 200' in msg:return 'null-body-status','Network response','Observed HTTP status was 200 rather than the expected no-body status.'
    if 'formatblock' in u and 'Wrong result returned' in msg and 'but got ""' in msg:
        return 'formatblock-query-empty','Editing','The operation returned an empty formatBlock queryCommandValue where a block tag name was expected.'
    if 'insertlinebreak' in u and '&nbsp;' in msg:
        return 'linebreak-leading-space','Editing','The observed markup introduced a nonbreaking space after the inserted line break.'
    if 'link-boundaries-insertion' in u and 'should be inserted into the link' in msg:
        return 'link-boundary-insertion','Editing','Text insertion at a link boundary left the link text unchanged.'
    if u.startswith('workers/modules/') and 'Static import' in n and 'expected true got false' in msg:
        return 'worker-static-data-import','Module loading','The dedicated-worker static import assertion returned false. The receipt does not identify the underlying fetch or module failure.'
    if 'form-submission-target/' in u and 'referrer expected ""' in msg:
        return 'targeted-form-referrer','Auxiliary browsing contexts','Targeted form submissions with noreferrer included a nonempty referrer. The row records the element and target attribute.'
    if 'moving-between-documents/' in u and 'Script load event fired unexpectedly' in msg:
        return 'moved-script-load-event','Script loading','A moved prepared script fired an unexpected load event.'
    if 'drawimage_crossorigin' in u and 'did not throw' in msg:
        return 'canvas-read-exception','Canvas and media','An image-drawing assertion expected a DOM exception on pixel readback; none was reported.'
    return 'unclassified-'+u.split('/')[0],'Unresolved observation','An observed assertion failed; inspect its exact diagnostic in the occurrence ledger.'

if __name__=='__main__':
    p=argparse.ArgumentParser()
    for k in ['final','out','even','odd','previous','chrome']:p.add_argument('--'+k,type=Path,required=True)
    run(p.parse_args())
