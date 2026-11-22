"""Export matched occurrence/page comparisons from checked, completed census shards."""
import argparse
from collections import Counter, defaultdict
from contextlib import ExitStack, contextmanager
import fcntl, gzip, hashlib, json, os, re, shutil, sqlite3, tempfile, zlib
from pathlib import Path
from reference_index import HEAD,TREE,PREVIOUS,WPT,MANIFEST,JSC,RUNNER,REFERENCE,PINNED,compact,read,require,sha
import wpt_ports
from tracked import Cohorts

PLAN_SHA='4a3badc8428aa0743c1747b1aecf70082d30e1a69cd9e6a17d8ce200eeb6a63e'
NORMALIZER_SHA='844d840fad23dc2890b5b6fba96de41a191a21f140df11c65310603d231459ba'
HISTORIC_BINARY='543a7b85e3ecd8b99400d8c0c6861ff4756621b106d86407ca9d9098ca9b0d4d'
STREAMS=('named','pages','gains','losses','issues')
def save(path,data):
    path.write_text(json.dumps(data,indent=1,sort_keys=True,ensure_ascii=True)+'\n')
@contextmanager
def gzout(path):
    with path.open('wb') as raw, gzip.GzipFile(filename='',fileobj=raw,mode='wb',mtime=0,compresslevel=3) as gz:
        yield lambda row: gz.write(compact(row)+b'\n')
def manifest(directory):
    files=sorted(p for p in directory.iterdir() if p.is_file() and p.name!='SHA256SUMS')
    (directory/'SHA256SUMS').write_text(''.join(sha(p)+'  '+p.name+'\n' for p in files))
def verify_manifest(directory):
    for line in (directory/'SHA256SUMS').read_text().splitlines():
        digest,name=line.split('  ',1)
        require(Path(name).name==name and sha(directory/name)==digest,'damaged export: '+name)

def port_map(log,port):
    require(sha(log)==port['logSHA256'],'wptserve log checksum mismatch')
    proof=port['proof'];require(re.fullmatch('[a-f0-9]{64}',proof['sha256']),'invalid port config hash')
    announce=re.findall(r'^wptserve on http://web-platform\.test:(\d+) \(log .*?/serve\.log\)$',log.read_text(errors='replace'),re.M)
    require(announce==[str(proof['ports']['http'][0])],'wrong server announcement')
    mapping={}
    for role,values in proof['ports'].items():
        if role not in wpt_ports.DEFAULTS:continue
        require(len(values)<=len(wpt_ports.DEFAULTS[role]),'unexpected port multiplicity')
        for actual,standard in zip(values,wpt_ports.DEFAULTS[role]):
            key=('https' if role=='h2' else role.split('-')[0],actual)
            require(key not in mapping,'duplicate port mapping')
            mapping[key]=standard
    return mapping

def score_named(dst,fst,snd):
    a=fst is not None and fst[0]=='Pass';b=snd is not None and snd[0]=='Pass'
    dst.update(firstPass=a,secondPass=b,commonPass=a and b,firstOnly=a and not b,secondOnly=b and not a,
               missingFirstName=fst is None and snd is not None,missingSecondName=snd is None and fst is not None,
               firstOnlyPreviouslyObserved=a and not b and snd is not None,
               firstOnlyNewlyObserved=a and snd is None,
               secondOnlyNowObserved=b and not a and fst is not None,
               secondOnlyNowMissing=b and fst is None)

def check_header(raw,identity,selector):
    expected={'wptRevision':WPT,'jobs':4,'kind':'testharness','dirs':['non-css'],'http2':True,
              'testdriver':True,'server':'wptserve','reuseContext':False,'manifestInventory':True,
              'generatedGlobals':True,'dedicatedWorkers':True,'restricted':True,'mode':'full',
              'label':identity.get('label','lp'),'bin':identity['bin']}
    for k,v in expected.items():require(raw.get(k)==v,f'bad receipt {k}: {raw.get(k)!r}')
    plan=raw['inventoryPlan']
    require(plan['manifestSha256']==MANIFEST and plan['wptRevision']==WPT,'manifest mismatch')
    require(plan['viewport']=={'width':800,'height':600,'deviceScaleFactor':1},'viewport mismatch')
    urls={r['url'] for r in raw['results']}; expected=set(selector)
    require(len(selector)==len(expected)==len(raw['results'])==len(raw['plan']) and set(raw['plan'])==urls==expected,'selection mismatch')
    return {r['url']:r for r in raw['results']}

def single(cache,db,t,selector,inventory,cohorts,run,ledger,ident,demo):
    key=t['key'];base=run/'raw';rawfile=base/(key+'.json');log=base/(key+'.log');portfile=run/'verified-ports'/(key+'.json')
    compressed=rawfile.with_suffix('.json.gz')
    require(rawfile.is_file() or compressed.is_file(),'missing raw receipt: '+key)
    packed_checksum=None
    if compressed.is_file():
        with gzip.open(compressed,'rb') as src:unpacked=hashlib.file_digest(src,'sha256').hexdigest()
        require(unpacked==ledger['receiptSHA256'],'invalid packed receipt digest: '+key)
        packed_checksum=sha(compressed)
    if rawfile.is_file():
        require(sha(rawfile)==ledger['receiptSHA256'],'invalid receipt digest: '+key)
    require(ledger['count']==t['count'],'bad receipt count: '+key)
    port=read(portfile)
    unresolved=port.get('proof') is None
    if unresolved:
        require(ident.get('machine')=='studio' and port.get('shard')==key and port.get('missing') and port['logSHA256']==sha(log),'missing unauthenticated port disposition')
        mapping={}
    else:
        mapping=port_map(log,port)
    config_digest=(port['proof'] or {}).get('sha256')
    for field,value in [('index',t['index']),('owner',t['owner']),('selectionSHA256',t['sha256']),('logSHA256',port['logSHA256']),('configSHA256',config_digest)]:
        if field in ledger:require(ledger[field]==value,'invalid receipt ledger '+field+': '+key)
    start_path=base/(key+'.start.json')
    if not demo and ident.get('machine')=='studio' and not start_path.is_file():
        require(ident.get('producerLedgerSHA256')==sha(run/'producer-completed.jsonl'),'missing authentic Studio completion ledger')
    elif not demo:
        require(start_path.is_file(),'missing launch evidence: '+key)
        start=read(start_path)
        for field,expected in [('key',key),('ordinal',t['index']),('selectorSHA256',t['sha256']),('artifactSHA256',ident['binarySHA256'])]:
            require(start.get(field)==expected,'wrong launch identity '+field+': '+key)
    proof={'key':key,'index':t['index'],'owner':t['owner'],'count':t['count'],'selectionSHA256':t['sha256'],
           'receiptSHA256':ledger['receiptSHA256'],'logSHA256':port['logSHA256'],'portProofSHA256':sha(portfile),
           'serverConfigSHA256':config_digest,'transportGzipSHA256':packed_checksum,
           'launchRecordSHA256':sha(start_path) if start_path.exists() else None,
           'machineProvenanceSHA256':sha(run/'machine-provenance.json'),'machine':ident,'historicalDemonstration':demo}
    if unresolved:proof['unresolvedPortMapping']=port
    cache_id=hashlib.sha256(compact([sha(Path(__file__)),sha(Path(__file__).with_name('reference_index.py')),sha(Path(__file__).with_name('tracked.py')),REFERENCE,proof])).hexdigest()
    dest=cache/(key+'.'+cache_id)
    if dest.exists():verify_manifest(dest);return dest
    tmp=Path(tempfile.mkdtemp(dir=cache,prefix=key+'.tmp.'))
    try:
        if rawfile.is_file():raw=read(rawfile)
        else:
            with gzip.open(compressed,'rb') as f:raw=json.load(f)
        records=check_header(raw,ident,selector)
        stats=defaultdict(Counter);previous_keys=set();chrome_keys=set();stalled=[];tracked=[]
        with ExitStack() as stack:
            emit={s:stack.enter_context(gzout(tmp/(s+'.jsonl.gz'))) for s in STREAMS}
            for url in sorted(selector):
                v=db.execute('select payload from pages where url=?',(url,)).fetchone()
                require(v is not None,'missing archived URL '+url)
                prev,stable,keys,hold,oldnames=json.loads(zlib.decompress(v[0]))
                require(prev is not None and stable is not None and not hold,'ineligible archived page '+url)
                require(keys[0]==key,'shard mismatch with prior selector')
                previous_keys.add(keys[0]);chrome_keys.add(keys[1].removeprefix('chrome/'))
                r=records[url];inv=inventory[url]
                # The pinned runner synthesizes terminal error placeholders after
                # a global stall without source/blob fields or a page observation.
                placeholder=(r['status']=='error' and r.get('error')=='runner stalled' and
                             r.get('harnessStatus') is None and not r['subtests'])
                for field in ('source','blob','environment'):
                    if placeholder and field in ('source','blob') and field not in r:continue
                    require(r.get(field)==inv[field],f'changed inventory {field}: {url}')
                if placeholder:stalled.append(url)
                require(inv['eligible'] and not inv['withheld'] and inv['scope']=='non-CSS','noneligible official URL '+url)
                current,collisions=wpt_ports.index(((s['name'],s['status']) for s in r['subtests']),mapping)
                require(not collisions,'canonical name collision '+url)
                errors={k:s.get('msg') for k,s in zip(current,r['subtests'])}
                previous={(n,i):a for n,i,a,b in oldnames if a is not None}
                chrome={(n,i):b for n,i,a,b in oldnames if b is not None}
                passed=sum(v[0]=='Pass' for v in current.values())
                page=[r['status'],r['harnessStatus'],bool(current) and passed==len(current) and r['status']=='complete' and r['harnessStatus']=='OK',passed,len(current)]
                emit['pages']([url,page,stable,prev,[key,keys[1],keys[0]],r.get('error')])
                for name,occ in sorted(cohorts.byurl.get(url,())):
                    tracked.append([url,name,occ,current.get((name,occ)),page,errors.get((name,occ))])
                if r['status']!='complete' or r['harnessStatus']!='OK':
                    emit['issues']([url,key,r['status'],r['harnessStatus'],r.get('error'),len(current)])
                for label,pg in zip(['current','chrome','previous'],[page,stable,prev]):
                    stats[label].update(pages=1,strictPages=int(pg[2]),passingNames=pg[3],observedNames=pg[4])
                    stats[label+'ByEnvironment:'+inv['environment']].update(pages=1,strictPages=int(pg[2]),passingNames=pg[3],observedNames=pg[4])
                    stats[label+'ByArea:'+url.split('/')[0]].update(pages=1,strictPages=int(pg[2]),passingNames=pg[3],observedNames=pg[4])
                    stats[label+'PageOutcomes'][pg[0]]+=1
                    stats[label+'HarnessOutcomes'][str(pg[1])]+=1
                for label,pg in [('currentChrome',stable),('currentPrevious',prev)]:
                    stats[label].update(matchedPages=1,firstOnlyPage=int(page[2] and not pg[2]),secondOnlyPage=int(pg[2] and not page[2]))
                for n,i in sorted(current.keys()|previous.keys()|chrome.keys()):
                    a=current.get((n,i));b=chrome.get((n,i));c=previous.get((n,i))
                    row=[url,n,i,a,b,c];emit['named'](row)
                    if a is not None or b is not None:score_named(stats['currentChrome'],a,b)
                    if a is not None or c is not None:score_named(stats['currentPrevious'],a,c)
                    if (a is not None and a[0]=='Pass')!=(c is not None and c[0]=='Pass'):
                        which='gains' if a is not None and a[0]=='Pass' else 'losses'
                        emit[which](row+[r['status'],r['harnessStatus'],errors.get((n,i)),key])
        if 'statuses' in ledger:require(ledger['statuses']==stats['currentPageOutcomes'],'outcome ledger mismatch: '+key)
        for field in ['restarts','watchdogKills','wallMs']:
            if field in ledger:require(ledger[field]==raw[field],'lifecycle ledger mismatch: '+key)
        native=[{'url':r['url'],'status':r['status'],'error':r.get('error')} for r in records.values()
                if r['status'] in ('hang','crash','panic') or 'withheld without replay' in (r.get('error') or '')]
        if native or raw['restarts'] or raw['watchdogKills']:
            proof['nativeEvent']={'observed':native,'restarts':raw['restarts'],'watchdogKills':raw['watchdogKills'],
                                  'unassignedNativeExitPossible':bool(raw['watchdogKills'] or raw['restarts']>len(native)),
                                  'retryPolicy':'frozen no-native-retry runner'}
        if stalled:proof['runnerStalledPlaceholders']=stalled
        proof.update(restarts=raw['restarts'],watchdogKills=raw['watchdogKills'],wallMs=raw['wallMs'],
                     previousReceiptKeys=sorted(previous_keys),chromeReceiptKeys=sorted(chrome_keys))
        save(tmp/'counts.json',stats);save(tmp/'provenance.json',proof);save(tmp/'tracked.json',tracked);manifest(tmp)
        os.rename(tmp,dest)
        return dest
    finally:
        if tmp.exists():shutil.rmtree(tmp)

def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--plans',type=Path,required=True)
    parser.add_argument('--reference-db',type=Path,required=True)
    parser.add_argument('--plan-sha',default=PLAN_SHA,help='pinned effective tasks.json SHA-256, including any approved revised holds')
    parser.add_argument('--prior',type=Path,help='prior census root, overrides default sibling')
    parser.add_argument('--loss-audit',type=Path,help='canonical losses.json for previous 106; optional on portable bundles')

    parser.add_argument('--inventory',type=Path,help='canonical frozen official inventory (defaults to sibling prior census evidence)')
    parser.add_argument('--run',action='append',type=Path,required=True)
    parser.add_argument('--owner',choices=['local','studio','all'],default='all')
    parser.add_argument('--keys',type=Path,help='JSON list of specific key strings')
    parser.add_argument('--cache',type=Path,required=True)
    parser.add_argument('--out',type=Path,required=True)
    parser.add_argument('--require-complete',action='store_true')
    parser.add_argument('--historical-demo',action='store_true',help='only preflight-000 against its original LP receipt')
    args=parser.parse_args()
    require(sha(Path(__file__).with_name('wpt_ports.py'))==NORMALIZER_SHA,'normalizer mismatch')
    require(sha(args.plans/'tasks.json')==args.plan_sha,'unrecognized census plan')
    tasks=read(args.plans/'tasks.json')
    prior=args.prior or args.plans.resolve().parent.parent/'census-1f1be0324'
    prior_plan=prior/'tasks.json'
    require(sha(prior_plan)=='db88ec59be1a3dd7395c14d4e6c08d5d1d88918d38959811c242d76a4437eadd','original task order mismatch')
    original=read(prior_plan)
    require(len(tasks)==len(original)==320,'wrong number of original shards')
    selectors={};owners={};omitted={}

    for i,t in enumerate(tasks):
        k=t['key'];selpath=args.plans/'selections'/(k+'.json')
        require(t['index']==i and t['owner']==('local' if i%2==0 else 'studio') and k not in selectors,'invalid ordinal/assignment')
        require(sha(selpath)==t['sha256'],'selector checksum mismatch '+k)
        sel=read(selpath);require(len(sel)==len(set(sel))==t['count'],'duplicate or missing URL '+k)
        require(not owners.keys()&set(sel),'URL shared between shards')
        owners.update(dict.fromkeys(sel,k));selectors[k]=sel
        origin=original[i];origin_path=prior/'selections'/(k+'.json')
        require(k==origin['key'] and t['originalSHA256']==origin['sha256']==sha(origin_path) and t['originalCount']==origin['count'],'changed prior shard')
        withheld={ex if isinstance(ex,str) else ex['url'] for ex in t['excluded']}
        origin_urls=read(origin_path);require(len(origin_urls)==len(set(origin_urls))==origin['count'],'invalid prior selection')
        require(withheld<=set(origin_urls) and len(withheld)==len(t['excluded']) and [u for u in origin_urls if u not in withheld]==sel,'effective plan must be a subtractive ordered projection')
        for ex in t['excluded']:
            url=ex if isinstance(ex,str) else ex['url'];omitted[url]=ex
    require(len(tasks)==320 and len(owners)+len(omitted)==30358 and not owners.keys()&omitted.keys(),'unexpected census scope')
    omission_file=args.plans/'omitted-variants.json'
    if omission_file.exists():
        if args.plan_sha==PLAN_SHA:require(sha(omission_file)=='55ac6be0632fbce224b1d68c154ca312974776719b39b3c2d7c933cb053b9434','omission reasons checksum mismatch')
        exclusions={x['url']:x for x in read(omission_file)}
        require(exclusions.keys()==omitted.keys(),'omission reason coverage mismatch')
        omitted=exclusions
    wanted={t['key'] for t in tasks if args.owner in ['all',t['owner']]}
    if args.keys:
        keys=read(args.keys);require(len(set(keys))==len(keys) and set(keys)<=wanted,'invalid requested keys');wanted=set(keys)
    if args.historical_demo:require(wanted=={'preflight-000'},'demo restricted to one historical shard')
    db=sqlite3.connect(args.reference_db.resolve().as_uri()+'?mode=ro',uri=True)
    metadata={k:json.loads(v) for k,v in db.execute('SELECT k,v FROM metadata')}
    require(metadata['pinned']==PINNED and metadata['identity']['version']=='158.0.8127.37','incorrect reference DB')
    inventory_path=args.inventory or prior/'evidence/original-official-inventory.jsonl.gz'
    require(sha(inventory_path)=='5264be8ad140bc12608da08e520fac162262b00c42c7e8189cba05e2843753c5','frozen inventory mismatch')
    with gzip.open(inventory_path,'rb') as stream:
        inventory={r['url']:r for r in map(json.loads,stream) if r['scope']=='non-CSS'}
    require(len(inventory)==31827 and owners.keys()<=inventory.keys(),'official inventory mismatch')
    tasks_by_key={t['key']:t for t in tasks}
    audit=args.loss_audit
    if audit is None:
        default_audit=prior.parent/'loss-audit-1f1be0324/losses.json'
        if default_audit.exists():audit=default_audit
    cohorts=Cohorts(prior/'evidence',audit)
    candidates={}
    for run in args.run:
        ident=read(run/'machine-provenance.json')
        if args.historical_demo:
            require(ident['head']==PREVIOUS and ident['binarySHA256']==HISTORIC_BINARY,'not the original demo binary')
        else:
            require(ident['head']==HEAD and ident['tree']==TREE,'source mismatch')
        require(ident['runnerSHA256']==RUNNER and bool(re.fullmatch('[a-f0-9]{64}',ident['binarySHA256'])),'invalid machine identity')
        if not args.historical_demo:
            require(ident['binarySHA256']=='9b2454c277a52452150d59b33c64c7d46f0206781c0068f5a6b0dd0631ffd87e','not the verified macOS arm64 release executable')
            require(ident['wpt']==WPT and ident['manifestSHA256']==MANIFEST and ident['effectiveManifestSHA256']==args.plan_sha,'machine WPT/selection mismatch')
            require(ident.get('machine',ident.get('owner')) in ('local','studio'),'unrecognized machine assignment')

        previous_lines=set();ledgerpath=run/'raw/completed.jsonl'
        ledgertext=''
        if ledgerpath.exists():
            with ledgerpath.open() as f:
                fcntl.flock(f,fcntl.LOCK_SH);ledgertext=f.read()
        for line in ledgertext.splitlines():
            if not line.strip():continue
            row=json.loads(line);k=row['key']
            require(k in selectors and k not in previous_lines,'unknown or repeated ledger key: '+k);previous_lines.add(k)
            if k not in wanted:continue
            require(k not in candidates,'conflicting run ownership: '+k)
            require(ident.get('machine',ident.get('owner'))==tasks_by_key[k]['owner'],'wrong machine ownership')
            candidates[k]=(run,row,ident)
    if args.require_complete:require(wanted==candidates.keys(),'requested shards still missing')
    args.cache.mkdir(parents=True,exist_ok=True)
    parts=[single(args.cache,db,t,selectors[t['key']],inventory,cohorts,*candidates[t['key']],args.historical_demo)
           for t in tasks if t['key'] in candidates]
    require(parts,'no eligible completed receipts')
    snapshot=hashlib.sha256(compact([args.plan_sha,cohorts.sources,sorted(wanted),[p.name for p in parts]])).hexdigest()
    if args.out.exists():
        verify_manifest(args.out)
        require(read(args.out/'summary.json')['snapshot']==snapshot,'different snapshot; choose a new --out directory')
        print(args.out);return
    args.out.parent.mkdir(parents=True,exist_ok=True)
    temp=Path(tempfile.mkdtemp(dir=args.out.parent,prefix=args.out.name+'.tmp.'))
    try:
        counts=defaultdict(Counter);provenance={};old={};chrome={};observations={};tracked_coverage={}
        for part in parts:
            meta=read(part/'provenance.json')
            provenance[meta['key']]=meta
            for url,name,occ,val,page,error in read(part/'tracked.json'):
                key=(url,name,occ);require(key not in observations,'duplicate tracked row');observations[key]=val,page,error
            old.update({k:metadata['previousReceipts'][k] for k in meta['previousReceiptKeys']})
            chrome.update({k:metadata['chromeReceipts'][k] for k in meta['chromeReceiptKeys']})
            for group,values in read(part/'counts.json').items():counts[group].update(values)
        for stream in STREAMS:
            with (temp/(stream+'.jsonl.gz')).open('wb') as out:
                for part in parts:
                    with (part/(stream+'.jsonl.gz')).open('rb') as src:shutil.copyfileobj(src,out)
        coverage=Counter();stalled={u for v in provenance.values() for u in v.get('runnerStalledPlaceholders',[])}
        with gzout(temp/'coverage.jsonl.gz') as emit:
            for (url,) in db.execute('SELECT url FROM pages ORDER BY url'):
                k=owners.get(url);reason=None
                if url in stalled:state='runnerStalledPlaceholder';reason='Error placeholder emitted by pinned runner without a page result'
                elif k in candidates:state='measured'
                elif k in wanted:state='requestedPending'
                elif k:state='outsideRequestedShardSet'
                elif url in omitted:
                    state='newlyWithheld'
                    reason=omitted[url] if isinstance(omitted[url],dict) else 'Withheld before execution by the pinned effective plan'
                    if isinstance(reason,dict):k=reason.get('shard')
                else:
                    state='historicallyWithheld'
                    blob=db.execute('SELECT payload FROM pages WHERE url=?',(url,)).fetchone()[0]
                    reason=json.loads(zlib.decompress(blob))[3]
                inv=inventory[url]
                if url in cohorts.byurl:tracked_coverage[url]=(state,k,reason)
                if state!='measured':counts['coverageByEnvironment:'+inv['environment']][state]+=1
                coverage[state]+=1;emit([url,state,k,reason,inv['environment'],inv['source'],inv['blob'],inv['guard'],inv['withheld']])
        pending=sorted(wanted-candidates.keys())
        require(coverage['measured']+coverage['runnerStalledPlaceholder']==counts['current']['pages'] and sum(coverage.values())==31827,'coverage totals inconsistent')
        summary=dict(head=PREVIOUS if args.historical_demo else HEAD,tree=None if args.historical_demo else TREE,
                     historicalDemonstration=args.historical_demo,referencePublication=REFERENCE,
                     previous=PREVIOUS,chrome=metadata['identity']['version'],wpt=WPT,manifestSHA256=MANIFEST,
                     planSHA256=args.plan_sha,scheduledUrls=len(owners),newlyWithheld=len(omitted),shardsScheduled=len(tasks),shardsRequested=len(wanted),
                     shardsComplete=len(parts),pendingKeys=pending,coverage=coverage,counts=counts,snapshot=snapshot,
                     requestedScopeComplete=not pending,fullCensusComplete=len(parts)==320 and not args.historical_demo)
        cohorts.export(temp,summary['head'],observations,tracked_coverage)
        save(temp/'summary.json',summary);save(temp/'current-provenance.json',provenance)
        save(temp/'previous-provenance.json',old);save(temp/'chrome-provenance.json',chrome)
        save(temp/'reference-identity.json',metadata['identity'])
        save(temp/'native-events.json',{k:v['nativeEvent'] for k,v in provenance.items() if 'nativeEvent' in v})
        manifest(temp)
        os.rename(temp,args.out);print(args.out)
    finally:
        if temp.exists():shutil.rmtree(temp)

if __name__=='__main__':main()
