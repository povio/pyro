"""Read-only independent validation of a cumulative export against original raw and frozen rows.

Example: python3 check-export.py SNAPSHOT --run LOCAL_RUN [--run STUDIO_RUN]
Only the requested QA result is written, not the export, plans, cache or raw runs.
"""
from pathlib import Path
import argparse,collections,gzip,hashlib,importlib.util,itertools,json,os,re,sys
a=Path(__file__).resolve().parent;root=a.parent;hist=root.parent/'census-1f1be0324/evidence'
def read(p):return json.loads(p.read_bytes())
def sha(p):
    with p.open('rb') as f:return hashlib.file_digest(f,'sha256').hexdigest()
def rows(p):
    with gzip.open(p,'rb') as f:yield from map(json.loads,f)
def sig(rows):
    h=hashlib.sha256()
    for x in rows:h.update(json.dumps(x,separators=(',',':'),ensure_ascii=True).encode()+b'\n')
    return h.hexdigest()
def require(ok,msg):
    if not ok:raise ValueError(msg)
normalizer=root.parents[1]/'round6-reference-158/census/canonical_ports.py'
require(sha(normalizer)=='48102949ce8412b426c763bd5bfcf194a425ce3123624b424abf2e0bcaf61a5b','normalizer')
spec=importlib.util.spec_from_file_location('official_canonical',normalizer);norm=importlib.util.module_from_spec(spec);spec.loader.exec_module(norm)
parser=argparse.ArgumentParser();parser.add_argument('export',type=Path);parser.add_argument('--run',action='append',type=Path,required=True)
parser.add_argument('--plans',type=Path,default=root/'plans-v2');parser.add_argument('--historical-demo',action='store_true');parser.add_argument('--out',type=Path,required=True);args=parser.parse_args();out=args.export
require(args.out.resolve().is_relative_to(a),'write outside audit')
for fixed,digest in [('pages.jsonl.gz','5e0ae28ea74e599ab87edd2fa851c5a1b9b6d65f137d4a50dafe61b609bdc9ec'),('named.jsonl.gz','9197a28dd9d8e52e210c5d7373f1f89642f58a92f398c4521e2d7ce70e5c959c')]:
    require(sha(hist/fixed)==digest,'prior source changed '+fixed)
for manifest_line in (out/'SHA256SUMS').read_text().splitlines():
    checksum,filename=manifest_line.split('  ',1)
    require(Path(filename).name==filename and sha(out/filename)==checksum,'export digest '+filename)
summary=read(out/'summary.json');require(summary['historicalDemonstration']==args.historical_demo and summary['head']==('1f1be032436bfeee9f48cdb5acbc0f71d21d5af9' if args.historical_demo else 'c3372ad21df84bb4bb7386ae4f114fa185d10f30'),'wrong head or demo identity')
prov=read(out/'current-provenance.json');counts=collections.defaultdict(collections.Counter)
require(sha(args.plans/'tasks.json')==summary['planSHA256'],'summary selection mismatch')
tasks={t['key']:t for t in read(args.plans/'tasks.json')}
inventory={r['url']:r for r in rows(hist/'original-official-inventory.jsonl.gz') if r['scope']=='non-CSS'}
official_urls=set(inventory)
target_urls=set(u for key in prov for u in read(args.plans/'selections'/(key+'.json')))
run_records={}
for run in args.run:
    for key,meta in prov.items():
        f=run/'raw'/(key+'.json')
        if not f.exists():f=run/'raw'/(key+'.json.gz')
        if f.exists():
            require(key not in run_records,'raw shards overlap');run_records[key]=(run,f)
require(set(run_records)==set(prov),'missing raw input')
priorpg={u:(lp,ch,ks) for u,lp,ch,_,ks,_ in rows(hist/'pages.jsonl.gz') if u in target_urls}
prevsig={}
for u,rowset in itertools.groupby(rows(hist/'named.jsonl.gz'),lambda r:r[0]):
    if u in target_urls:prevsig[u]=sig((r[1],r[2],r[3],r[4]) for r in rowset if r[3] is not None or r[4] is not None)
coverage={}
for row in rows(out/'coverage.jsonl.gz'):
    u,state,k,reason,env,source,blob,guards,previous_holds=row
    require(u not in coverage,'duplicate official URL')
    inv=inventory[u];require((env,source,blob,guards,previous_holds)==tuple(inv[f] for f in ['environment','source','blob','guard','withheld']),'official coverage metadata drift '+u)
    coverage[u]=state
    if state!='measured':counts['coverageByEnvironment:'+env][state]+=1

require(set(coverage)==official_urls,'official coverage universe')
require({u for u,v in coverage.items() if v in ('measured','runnerStalledPlaceholder')}==target_urls,'wrong measured coverage')
require({u for u,v in coverage.items() if v=='newlyWithheld'}==set(read(args.plans/'omit-urls.json')),'wrong new hold coverage')
require(collections.Counter(coverage.values())==summary['coverage'],'coverage summary mismatch')
trackfiles={'historical292.json':292,'previous106.json':106}
keyrow=lambda r:(r['url'],r['canonical'],r['occurrence'])
tracked={f:{keyrow(r['original']):r for r in read(out/f)} for f in trackfiles}
require(all(len(tracked[f])==n for f,n in trackfiles.items()),'historical tracking cardinality')
require({keyrow(r) for r in read(hist/'historical-292.json')}==set(tracked['historical292.json']),'292 original identity')
require({tuple(r[:3]) for r in rows(hist/'prior-pass-changes.jsonl.gz') if r[3]=='loss'}==set(tracked['previous106.json']),'106 original identity')
byurl=collections.defaultdict(set)
for dataset in tracked.values():
    for u,n,i in dataset:byurl[u].add((n,i))
rawtracked={}
groups=iter(itertools.groupby(rows(out/'named.jsonl.gz'),lambda r:r[0]));group=next(groups,None)
change_iters={k:iter(rows(out/(k+'.jsonl.gz'))) for k in ['gains','losses','issues']}
seen=set();last=None;rawrows={};nrows=0
for u,head,ch,prev,keys,error in rows(out/'pages.jsonl.gz'):
    k=keys[0];require(k in prov and u in target_urls and u not in seen,'wrong/duplicate page '+u);seen.add(u)
    if k!=last:
        run,f=run_records[k];meta=prov[k];t=tasks[k];last=k
        require(meta['index']==t['index'] and meta['owner']==t['owner'],'wrong assigned owner')
        rawbytes=gzip.open(f,'rb').read() if f.suffix=='.gz' else f.read_bytes()
        require(hashlib.sha256(rawbytes).hexdigest()==meta['receiptSHA256'] and meta['selectionSHA256']==t['sha256'],'bad raw/selection digest')
        raw=json.loads(rawbytes);del rawbytes;rawrows={r['url']:r for r in raw['results']}
        require(len(rawrows)==len(raw['results'])==t['count'] and set(rawrows)==set(read(args.plans/'selections'/(k+'.json'))),'raw URL mismatch')
        require(raw['jobs']==4 and raw['mode']=='full' and raw['wptRevision']==summary['wpt'] and raw['inventoryPlan']['manifestSha256']==summary['manifestSHA256'] and raw['http2'] and raw['testdriver'] and raw['server']=='wptserve' and raw['reuseContext']==False,'bad matched profile')
        require(raw['bin']==meta['machine']['bin'] and raw['label']==meta['machine'].get('label','lp'),'binary path/label mismatch')
        if not args.historical_demo:
            localident=read(run/'machine-provenance.json');require(localident==meta['machine'] and localident['tree']==summary['tree'] and localident['head']==summary['head'],'machine identity drift')
            require((localident['machine'],localident['effectiveManifestSHA256'])==(t['owner'],summary['planSHA256']),'machine role/plan mismatch')
        pp=run/'verified-ports'/(k+'.json');port=read(pp);mp={}
        require(sha(pp)==meta['portProofSHA256'] and sha(run/'raw'/(k+'.log'))==port['logSHA256'],'port provenance')
        for role,actual in port['proof']['ports'].items():
            if role in norm.DEFAULTS:
                for source,dest in zip(actual,norm.DEFAULTS[role]):mp['https' if role=='h2' else role.split('-')[0],source]=dest
    rr=rawrows[u];sub=rr['subtests'];num=sum(t['status']=='Pass' for t in sub)
    expected=[rr['status'],rr.get('harnessStatus'),bool(sub) and rr['status']=='complete' and rr.get('harnessStatus')=='OK' and num==len(sub),num,len(sub)]
    require(expected==head and error==rr.get('error') and (prev,ch)==priorpg[u][:2] and keys==[k,priorpg[u][2][1],priorpg[u][2][0]],'page details differ '+u)
    stalled=head[0]=='error' and error=='runner stalled' and not sub and head[1] is None
    require((coverage[u]=='runnerStalledPlaceholder')==stalled,'wrong runner stall coverage '+u)
    if head[0]!='complete' or head[1]!='OK':require(next(change_iters['issues'],None)==[u,k,head[0],head[1],error,head[4]],'issue '+u)
    rawkeys={};occ=collections.Counter()
    for item in sub:
        canon=norm.canonical_name(item['name'],mp);occ[canon]+=1;rawkeys[canon,occ[canon]]=[item['status'],item['name'],item.get('msg')]
    for name,i in byurl.get(u,()):
        obs=rawkeys.get((name,i));rawtracked[u,name,i]=(obs[:2] if obs else None,head,obs[2] if obs else None)
    smaller=[];keys_seen=set()
    if group and group[0]==u:
        for _,name,i,current,stable,previous in group[1]:
            nrows+=1;ki=(name,i);require(ki not in keys_seen,'duplicate exported name');keys_seen.add(ki)
            observed=rawkeys.pop(ki,None)
            require((observed[:2] if observed else None)==current,'current named status/original mismatch '+u)
            if stable is not None or previous is not None:smaller.append([name,i,previous,stable])
            for label,second in [('currentChrome',stable),('currentPrevious',previous)]:
                if current is None and second is None:continue
                h=bool(current and current[0]=='Pass');b=bool(second and second[0]=='Pass')
                counts[label].update(firstPass=h,secondPass=b,firstOnly=h and not b,secondOnly=b and not h,commonPass=h and b,missingFirstName=current is None,missingSecondName=second is None,firstOnlyPreviouslyObserved=h and not b and second is not None,firstOnlyNewlyObserved=h and second is None,secondOnlyNowObserved=b and not h and current is not None,secondOnlyNowMissing=b and current is None)
            h=bool(current and current[0]=='Pass');b=bool(previous and previous[0]=='Pass')
            if h!=b:
                direction='gains' if h else 'losses'
                row=[u,name,i,current,stable,previous,head[0],head[1],observed[2] if observed else None,k]
                require(next(change_iters[direction],None)==row,direction+' '+u)
        group=next(groups,None)
    require(not rawkeys and sig(smaller)==prevsig.get(u,sig([])),'prior/reference occurrence mismatch '+u)
    for label,col in [('current',head),('chrome',ch),('previous',prev)]:
        counts[label].update(pages=1,strictPages=col[2],passingNames=col[3],observedNames=col[4])
        for bucket in [label+'ByEnvironment:'+inventory[u]['environment'],label+'ByArea:'+u.split('/')[0]]:
            counts[bucket].update(pages=1,strictPages=col[2],passingNames=col[3],observedNames=col[4])
        counts[label+'PageOutcomes'][col[0]]+=1;counts[label+'HarnessOutcomes'][str(col[1])]+=1
    for label,col in [('currentChrome',ch),('currentPrevious',prev)]:
        counts[label].update(matchedPages=1,firstOnlyPage=head[2] and not col[2],secondOnlyPage=col[2] and not head[2])
require(seen==target_urls and group is None,'incomplete output')
for direction,it in change_iters.items():require(next(it,None) is None,'extra '+direction)
for name,c in counts.items():require(c==summary['counts'][name],name+' comparison counters disagree')
historical_counts={}
for f,dataset in tracked.items():
    totals=collections.Counter()
    for ki,r in dataset.items():
        status,page,error=rawtracked.get(ki,(None,None,None));state=coverage[ki[0]]
        disposition=('missingName' if status is None else status[0]) if state=='measured' else state
        require([status,page,error,state,disposition,summary['head']]==[r['current'],r['currentPage'],r['currentSubtestDiagnostic'],r['coverage'],r['currentState'],r['currentHead']],'historical disposition differs')
        totals[disposition]+=1
    historical_counts[f]=totals
    require(totals==read(out/'historical-tracking.json')['summary'][f.removesuffix('.json')]['currentDisposition'],'historical summary drift')
qa={'historicalTracking':historical_counts,'historicalDemonstration':args.historical_demo,'exportSnapshot':summary['snapshot'],'head':summary['head'],'shards':len(prov),'urls':len(seen),'occurrenceRows':nrows,'verified':True,'areasChecked':['frozen original names and occurrences for both comparators','new raw names and statuses for every measured URL','all directional gains/losses and diagnostics','per-page outcomes, setup diagnostics and strict flags','coverage, historical and added holds','canonical archived server ports','independently recomputed directional and gross counts'],'counts':counts}
if out.name=='final-local':
    local={k for k,t in tasks.items() if t['index']%2==0}
    require(sum(t['count'] for k,t in tasks.items() if k in local)==15072 and len(local)==160 and set(prov)==local,'local assignment incomplete or wrong owner')
    require(sorted(tasks[k]['index'] for k in prov)==list(range(0,320,2)) and all(tasks[k]['owner']=='local' for k in prov),'unexpected Studio ordinal')
    require(len(seen)==15072 and summary['requestedScopeComplete'] and not summary['fullCensusComplete'],'local completion flag mismatch')
    qa['localAssignment']={'verified':True,'shards':160,'urls':len(seen),'ordinalRule':'zero-based even ordinals, 0 through 318','studioOrdinals':0,'duplicateKeys':0,'duplicateUrls':0,'selectorSHA256':sha(args.plans/'local-tasks.json'),'independentBuildReadbackSHA256':sha(a/'local-build-readback-v2.json')}
args.out.write_text(json.dumps(qa,indent=1)+'\n')
print(len(prov),len(seen),nrows,summary['snapshot'])
