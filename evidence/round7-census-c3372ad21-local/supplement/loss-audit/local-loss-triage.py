"""Join saved canonical local losses to signed, current and previous raw observations."""
from pathlib import Path
import collections,gzip,hashlib,importlib.util,json,re
here=Path(__file__).resolve().parent
root=here.parent
final=root/'final-local'
cache=root.parents[1]
old=root.parent/'census-1f1be0324'
chrome=cache/'round6-reference-158/census'
def j(p): return json.loads(p.read_bytes())
def rows(p):
    with gzip.open(p,'rb') as stream:yield from map(json.loads,stream)
def digest(p):
    with p.open('rb') as stream:return hashlib.file_digest(stream,'sha256').hexdigest()
def save(p,obj):p.write_text(json.dumps(obj,ensure_ascii=True,indent=1)+'\n')
def check(expr,message):
    if not expr: raise ValueError(message)
meta={tag:j(final/f'{file}-provenance.json') for tag,file in [('current','current'),('prior','previous'),('signed','chrome')]}
normalizer=chrome/'export-final/canonical_ports.py'
check(digest(normalizer)=='48102949ce8412b426c763bd5bfcf194a425ce3123624b424abf2e0bcaf61a5b','normalizer drift')
sp=importlib.util.spec_from_file_location('canon',normalizer);canon=importlib.util.module_from_spec(sp);sp.loader.exec_module(canon)
losses=list(rows(final/'losses.jsonl.gz'))
expected={tuple(r[:3]) for r in rows(final/'named.jsonl.gz') if r[5] and r[5][0]=='Pass' and (r[3] is None or r[3][0]!='Pass')}
check(len(losses)==len(expected)==74 and expected=={tuple(r[:3]) for r in losses},'loss union mismatch')
urls={r[0] for r in losses};pg={r[0]:r for r in rows(final/'pages.jsonl.gz') if r[0] in urls}
def signature(u,n,diagnostic):
    if u.startswith('html/cross-origin-opener-policy/') and 'Popup name is cleared?' in diagnostic:return 'popup-name-not-cleared'
    if 'dangling-markup' in u:return 'dangling-target-name-stash'
    if 'clear-window-name' in u:return 'navigation-name-stash'
    if 'rel-button-target' in u:return 'form-target-referrer'
    if 'window-open-noopener-existing-iframe' in u:return 'named-iframe-timeout'
    if 'formatblock' in u:return 'formatblock-value' if 'Wrong result returned' in diagnostic else 'formatblock-merged-siblings'
    if 'inserttext' in u:return 'text-after-link'
    if 'plaintext-only-in-designMode' in u:return 'plaintext-designmode-bold'
    if 'createImageBitmap-origin' in u:return 'video-image-decode'
    if 'drawImage-ImageBitmap-cloned' in u:return 'cloned-image-timeout'
    if 'canvas-display-p3-drawImage' in u:return 'transparent-pixel'
    if 'response-null-body' in u:return 'null-body-status'
    if 'query-encoding/' in u:return 'embedded-image-width'
    if 'inert-iframe' in u:return 'inert-frame-focus'
    if 'inert-and-find' in u:return 'flat-tree-text-search'
    if 'text-module/service-worker-test' in u:return 'sw-text-import-loaded'
    if 'dedicated-worker-import-data-url' in u:return 'worker-static-data-import'
    if 'file-system-access/opaque-origin' in u:return 'opaque-window-interface'
    if 'ctor-analyser' in u:return 'analyser-diagnostic-name'
    if 'protocol.https' in u:return 'protocol-port-name'
    if 'early-hints-preconnect' in u:return 'early-hints-title-name'
    raise ValueError(u)
labels={
'popup-name-not-cleared':('Auxiliary browsing contexts','COOP iframe-opened popup retained its name where the assertion expected it cleared.'),
'dangling-target-name-stash':('Auxiliary browsing contexts','Target-name round-trip stash result differed for base/form/link target cases. The failure message does not expose the returned value.'),
'navigation-name-stash':('Auxiliary browsing contexts','Cross-origin navigation with noopener returned an unexpected stash value. The raw assertion does not identify the wrong field.'),
'form-target-referrer':('Auxiliary browsing contexts','Button-targeted form submissions with noreferrer sent a nonempty referrer.'),
'named-iframe-timeout':('Auxiliary browsing contexts','The noopener/named-iframe case timed out before a final opener assertion.'),
'formatblock-value':('Editing','formatBlock returned an empty query value instead of div.'),
'formatblock-merged-siblings':('Editing','formatBlock produced one div with a br instead of two sibling divs.'),
'text-after-link':('Editing','Insertion at the end of a link placed the new character outside the link.'),
'plaintext-designmode-bold':('Editing','Bold markup was absent from the designMode/plaintext-only result.'),
'video-image-decode':('Canvas and media','createImageBitmap rejected two cross-origin/redirect video sources with an InvalidStateError decode diagnostic.'),
'cloned-image-timeout':('Canvas and media','The cloned ImageBitmap subtest timed out on a page reporting a harness error.'),
'transparent-pixel':('Canvas and media','A drawImage pixel was transparent black instead of opaque red.'),
'null-body-status':('Network response','Worker fetch observed 200 for the expected 204 POST/304 OPTIONS response.'),
'embedded-image-width':('Element resource loading','The query-encoding embed probe reported fallback width 300 instead of image width 2. This alone does not establish a URL encoding failure.'),
'inert-frame-focus':('Focus and text traversal','Sequential focus remained on the preceding element rather than the expected iframe.'),
'flat-tree-text-search':('Focus and text traversal','Text searches inside shadow-root dialogs returned false, including a slotted-text case.'),
'sw-text-import-loaded':('Module loading','A service-worker text import reported LOADED when the assertion expected FAILED.'),
'worker-static-data-import':('Module loading','The dedicated-worker static data-URL import assertion returned false; no lower-level diagnostic was supplied.'),
'opaque-window-interface':('Window interface exposure','A data-URL iframe exposed FileSystemDirectoryHandle when the assertion expected it absent.'),
'analyser-diagnostic-name':('Recorded name changes','A still-passing constructor error case includes different TypeError message text in its generated subtest name.'),
'protocol-port-name':('Recorded name changes','A literal %s followed by a dynamic wptserve port appears in this generated name; the unchanged canonicalizer does not rewrite this token.'),
'early-hints-title-name':('Recorded name changes','The old filename-derived passing name was absent. Current LP and signed Chrome both reported a descriptive failing assertion on the same complete page.')}
def alternate(sig,n,entries):
    if sig=='protocol-port-name':
        pattern=lambda t:re.sub(r'(?<=:%s)\d+', '<port>',t)
        return [s for s in entries if pattern(s['name'])==pattern(n)]
    if sig=='analyser-diagnostic-name':
        start=n.split(' threw TypeError:',1)[0]+' threw TypeError:'
        return [s for s in entries if s['name'].startswith(start)]
    if sig=='early-hints-title-name':
        return [s for s in entries if 'Only the first of multiple 103 Early Hints' in s['name']]
    return []
# Read only the proved filenames and preserve raw reported names/diagnostics.
cachepages={};proofs={}
for u,row in pg.items():
    for tag,i,base,hashkey in [('current',0,root,'receiptSHA256'),('prior',2,old,'receiptSHA256'),('signed',1,chrome,'rawSHA256')]:
        key=row[4][i].removeprefix('chrome/');f=base/'raw'/(key+'.json')
        k=tag,key
        if k not in proofs:
            proven=meta[tag][key][hashkey];check(digest(f)==proven,tag+' checksum '+key)
            doc=j(f);port=j(base/'verified-ports'/(key+'.json'))['proof']['ports'] if tag=='current' else meta[tag][key]['ports'] if tag=='prior' else None
            # Signed noncanonical spelling can be cross-checked against the exact
            # original strings in the immutable public canonical ledger directly.
            mapping={}
            if port:
                for role,values in port.items():
                    if role not in canon.DEFAULTS:continue
                    for v,dest in zip(values,canon.DEFAULTS[role]):mapping['https' if role=='h2' else role.split('-')[0],v]=dest
            for rec in doc['results']:
                if rec['url'] in urls:cachepages[tag,rec['url']]=(key,proven,rec,mapping)
            proofs[k]=proven
records=[];groups=collections.defaultdict(list);seen={}
for u,n,i,head,stable,previous,page_status,harness_status,msg,key in losses:
    group=signature(u,n,msg or '');family,interpretation=labels[group];status=head[0] if head else 'Missing'
    item={'url':u,'canonicalName':n,'occurrence':i,'behaviorFamily':family,'signature':group,'observationClass':('missing exact name' if not head else 'observed subtest timeout' if status=='Timeout' else 'observed subtest failure'),'diagnosticInterpretation':interpretation,'implementationCause':'Unverified; isolated receipts establish the reported observation.','engines':{}}
    for tag,cell,pgi in [('current',head,1),('prior',previous,3),('signed',stable,2)]:
        shard,h,raw,mapping=cachepages[tag,u];subs=raw['subtests']
        if cell is not None:
            candidates=[s for s in subs if s['name']==cell[1]] if tag=='signed' else [s for s in subs if canon.canonical_name(s['name'],mapping)==n]
            check(len(candidates)>=i,(tag,u,n,i))
            r=candidates[i-1];assert [r['status'],r['name']]==cell
        else:
            r=None
            if tag!='signed':check(not [s for s in subs if canon.canonical_name(s['name'],mapping)==n][i-1:],(tag,u,n))
            else:check(not [s for s in subs if s['name']==n][i-1:],(tag,u,n))
        outpage=pg[u][pgi];check(outpage[:2]==[raw['status'],raw.get('harnessStatus')],(tag,u,'page'))
        item['engines'][tag]={'receiptKey':shard,'receiptSHA256':h,'lifecycle':outpage[0],'harnessStatus':outpage[1],'strictPage':outpage[2],'pagePassingNames':outpage[3],'pageObservedNames':outpage[4],'pageDiagnostic':raw.get('error'),'subtestStatus':r['status'] if r else None,'originalReportedName':r['name'] if r else None,'subtestDiagnostic':r.get('msg') if r else None}
        if group in ('analyser-diagnostic-name','protocol-port-name','early-hints-title-name'):
            alts=alternate(group,n,subs) if tag!='prior' else []
            if alts:item['engines'][tag]['samePageRelatedObservations']=[{'originalReportedName':s['name'],'subtestStatus':s['status'],'subtestDiagnostic':s.get('msg')} for s in alts]
    check(item['engines']['current']['subtestDiagnostic']==msg and previous[0]=='Pass',(u,n,'loss mismatch'))
    groups[group].append(item);records.append(item)
def stat(rs):
    return {'n':len(rs),'urls':len({r['url'] for r in rs}),'currentStatus':dict(collections.Counter(r['engines']['current']['subtestStatus'] or 'Missing' for r in rs)),'signedStatus':dict(collections.Counter(r['engines']['signed']['subtestStatus'] or 'Missing' for r in rs))}
bygroup=[dict(signature=g,family=labels[g][0],interpretation=labels[g][1],**stat(rs),rawDiagnostics=dict(collections.Counter(str(r['engines']['current']['subtestDiagnostic']) for r in rs))) for g,rs in sorted(groups.items(),key=lambda kv:(-len(kv[1]),kv[0]))]
summary=stat(records);summary['pagesByLifecycleAndHarness']=dict(collections.Counter(f"{rw[1][0]}/{rw[1][1]}" for rw in pg.values()));summary['observationsByLifecycleAndHarness']=dict(collections.Counter(f"{r['engines']['current']['lifecycle']}/{r['engines']['current']['harnessStatus']}" for r in records))
independent=j(root/'audit/checkpoints/final-local.json');check(independent['counts']['currentPrevious']['secondOnly']==len(records) and independent['counts']['currentPrevious']['secondOnlyNowMissing']==summary['currentStatus']['Missing'],'independent loss totals')
doc={'scope':'local even ordinals only','head':j(final/'summary.json')['head'],'previousLP':j(final/'summary.json')['previous'],'signedChrome':j(final/'summary.json')['chrome'],'wpt':j(final/'summary.json')['wpt'],'planSHA256':j(final/'summary.json')['planSHA256'],'rawReceiptDigestsVerified':len(proofs),'sources':{n:digest(final/n) for n in ['named.jsonl.gz','pages.jsonl.gz','losses.jsonl.gz','current-provenance.json','previous-provenance.json','chrome-provenance.json']},'independentLocalExportQA':digest(root/'audit/checkpoints/final-local.json'),'summary':summary,'signatureGroups':bygroup,'records':records}
save(here/'local-loss-triage.json',doc)
lines=['# Local prior-pass loss triage','','All 74 occurrences across 32 URLs were reconstructed from the immutable final named ledger, independently checked against the published loss stream, then joined to current, previous LP and signed Chrome raw receipts. The JSON retains the full original names, occurrence, exact statuses, diagnostics, three receipt checksums and per-engine page/harness states. No affected page was replayed.','', 'Current LP reported 66 named failures on complete/OK pages, two named timeouts and six missing exact names. One timeout occurred on a timeout/Timeout page; the other occurred on an error/Error cloned ImageBitmap page. Both previous and current LP reported the same page-level image-decode rejection there, although this particular name previously passed. The named-frame timeout previously completed and passed. There is no positive transport-failure diagnostic for either case. All six missing names occurred on complete/OK pages. There were no runner-stall placeholders among these 74. Signed Chrome passed 52, failed 12 and had no matching name for 10.','', '| Diagnostic group | Occurrences | URLs | Recorded behavior |','|---|---:|---:|---|']
for g in bygroup:lines.append(f"| {g['signature']} | {g['n']} | {g['urls']} | {g['interpretation']} |")
lines+=['','The largest exact diagnostic spans 36 popup name-clearing failures on 11 URLs. The six stash mismatches and three form referrer assertions have distinct reported symptoms. The two named timeouts lack a completed assertion. None of these observations assigns causality to an individual commit.','', 'Three missing audio constructor names were regenerated with changed TypeError text. The corresponding calls pass under their new names. Two missing protocol-handler names include the raw run-specific port after an unescaped %s token; the equivalent calls pass separately in all three receipts. One old early-hints name matched the filename; current LP and signed Chrome both reported a descriptive failing assertion. These related observations do not change the original occurrence keys or scores.','']
(here/'local-loss-triage.md').write_text('\n'.join(lines))
print(summary,len(proofs),[(g['signature'],g['n']) for g in bygroup])
