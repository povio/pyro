"""Build the guarded aa0268 comparison from recorded WPT observations, without launching a browser."""
import collections,gzip,hashlib,json,pathlib,sqlite3,sys,zlib
P=pathlib.Path
r=P(sys.argv[1]) if len(sys.argv)>1 else P.home()/'Library/Caches/lightningpanda/round6-noncss-measure'; run=r/'merged-aa0268'; work=run/'readback/work'; out=run/'readback/evidence'; plat=r/'platform-batch'; css=r/'css-reconciliation-572c'; non=r/'reconciled';root=r/'manifest-runs'; census=root/'census-902fe811f';pin='a421f2fea5eb7f91e0db399e44b0fb0779ee1603'
sys.path.insert(0,str(census))
from wpt_ports import endpoint_map,config,index
load=lambda f:json.loads(f.read_bytes());sha=lambda f:hashlib.sha256(f.read_bytes()).hexdigest()
def gz(f):return json.loads(gzip.open(f,'rb').read())
def dump(x):return json.dumps(x,separators=(',',':'),ensure_ascii=True)
def save(name,x):(out/name).write_text(json.dumps(x,indent=2,ensure_ascii=True)+'\n')
def put(f,x):print(dump(x),file=f)
def receipt(src,log,label,count,jobs):
 d=load(src);rows=d['results'];m,proof=config(str(log));assert 'error' not in proof,proof
 assert d['wptRevision']==pin and d['jobs']==jobs and d['testdriver'] and d['server']=='wptserve' and len(rows)==len(set(t['url'] for t in rows))==count and set(d['plan'])=={t['url'] for t in rows}
 assert P(d['bin']).resolve()==dict(merged=(run/'pyro'),prior52a=plat/'integration-a414',prior35df=plat/'final')[label].resolve()
 inproof[src.name]={'binary':label,'receiptSHA256':sha(src),'serverLogSHA256':sha(log),'serverConfigSHA256':proof['sha256'],'jobs':jobs,'urls':count,'http2':bool(d.get('http2')),'manifestInventory':bool(d.get('manifestInventory')),'restarts':d.get('restarts'),'watchdogKills':d.get('watchdogKills'),'statuses':dict(collections.Counter(t['status'] for t in rows))}
 configs[proof['sha256']]=proof['ports']
 return {t['url']:(t,m,proof['sha256']) for t in rows}
inproof={};configs={}
definitions={'loss':(non,'platform-loss-final',plat/'loss-final.log',159,4),'main':(non,'platform-rebase-a414',plat/'rebase-a414.log',2407,4),'extra':(non,'platform-extra-rebase-a414',plat/'extra-rebase-a414.log',38,4),'css':(css,'rebase-a414-css',css/'rebase-a414-css.log',1050,2),'workers':(css,'rebase-a414-extra',css/'rebase-a414-extra.log',41,2)}
old={};new={};ref52={};prior35={}
for cohort,(folder,prior,oldlog,count,jobs) in definitions.items():
 was=old[cohort]=receipt(folder/(prior+'.json'),oldlog,'prior35df' if cohort=='loss' else 'prior52a',count,jobs)
 now=new[cohort]=receipt(folder/('merged-aa-'+cohort+'.json'),run/(cohort+'.log'),'merged',count,jobs)
 assert now.keys()==was.keys(),cohort
 if cohort=='loss':prior35.update(was)
 else:
  assert not set(ref52).intersection(was)
  ref52.update(was)
for key,file in [('loss',plat/'loss-urls.json'),('main',plat/'selection.json'),('extra',plat/'extra.json'),('css',plat/'a414-css-selection.json')]:assert set(load(file))==set(new[key])
assert set(load(css/'supplement.json'))==set(new['workers'])
pub={s:gz(work/(s+'.json.gz')) for s in ['stable','canary']};sources=gz(work/'sources.json.gz')
sourcefiles={v[0]:v[1] for v in sources.values()}
for src,blob in sourcefiles.items():
 body=(r.parent/'wptci/wpt-full'/src).read_bytes();assert hashlib.sha1(b'blob '+str(len(body)).encode()+b'\0'+body).hexdigest()==blob,src
db=sqlite3.connect((root/'scores.sqlite').as_uri()+'?mode=ro&immutable=1',uri=True)
labels=['merged','prior52a','prior35df','localStable','publicStable','publicCanary']
stablePaths={};stableInputRows={};cohortStats={};hist=load(r/'loss-audit-batch2/canonical-losses.json');byHist=collections.defaultdict(list)
for h in hist:byHist[h['url']].append(h)
historical=[]
def shape(raw,m=None,public=False):
 if raw is None:return None,{}
 if public:
  values=raw['subtests']; hs=raw['status']; page=hs;h=None
 else:
  row,m,h=raw;values=[(s['name'],s['status']) for s in row['subtests']];hs=row['harnessStatus'];page=row['status']
 names,collision=index(values,m or {});assert not collision
 passed=sum(v[0].upper()=='PASS' for v in names.values())
 full=bool(names) and hs=='OK' and (public or page=='complete') and passed==len(names)
 return [page,hs,full,len(names),passed,h],names
def addCounts(cc,a):
 if a is None:cc['missing']+=1;return
 cc.update(observed=1,fully=int(a[2]),names=a[3],namedPass=a[4]);cc['page_'+a[0]]+=1
with gzip.open(out/'pages.jsonl.gz','wt',compresslevel=4) as pf,gzip.open(out/'names.jsonl.gz','wt',compresslevel=4) as nf,gzip.open(out/'prior-changes.jsonl.gz','wt',compresslevel=4) as diff:
 for cohort,rows in new.items():
  cc=[collections.Counter() for _ in labels];pairs={l:collections.Counter() for l in labels[1:]}; ps={}
  for u in sorted(rows):
   source,blob,unchanged=sources[u]
   current=rows[u];assert not blob or current[0].get('blob',blob)==blob,(cohort,u,blob)
   ch=db.execute("select raw,path from result where label='chromeWpt' and url=?",(u,)).fetchone()
   local=None
   if ch:
    row=json.loads(zlib.decompress(ch[0]));mp,pr=endpoint_map(ch[1],u);assert 'error' not in pr,pr
    assert row['blob']==blob;configs[pr['sha256']]=pr['ports'];local=(row,mp,pr['sha256'])
    original=P(ch[1]);sourcekey=str(original.relative_to(root));logkey=str(P(pr['log']).relative_to(root))
    if sourcekey not in stablePaths:stablePaths[sourcekey]=sha(original)
    stableInputRows[u]=[sourcekey,logkey,pr['sha256']]
   prev52=ref52.get(u)
   prev35=prior35.get(u) if cohort=='loss' else None
   shapes=[shape(current),shape(prev52),shape(prev35),shape(local),shape(pub['stable'].get(u),public=True),shape(pub['canary'].get(u),public=True)]
   pages=[x[0] for x in shapes];maps=[x[1] for x in shapes]
   for i,v in enumerate(pages):addCounts(cc[i],v)
   put(pf,[cohort,u,source,blob,unchanged,*pages])
   for i,l in enumerate(labels[1:],1):
    a=pages[i];c=pairs[l]
    if a is None:c['missingReferencePage']+=1;continue
    c.update(paired=1,onlyHeadFull=int(pages[0][2] and not a[2]),onlyRefFull=int(a[2] and not pages[0][2]))
   for name,i in sorted(set().union(*maps)):
    vals=[m.get((name,i)) for m in maps];statuses=[v[0] if v else None for v in vals];passes=[s is not None and s.upper()=='PASS' for s in statuses]
    put(nf,[cohort,u,name,i,*vals])
    for j,l in enumerate(labels[1:],1):
     if pages[j] is not None:
      pairs[l].update(onlyHead=int(passes[0] and not passes[j]),onlyRef=int(passes[j] and not passes[0]),both=int(passes[0] and passes[j]),headMissingName=int(vals[0] is None and vals[j] is not None),referenceMissingName=int(vals[j] is None and vals[0] is not None))
    primary=2 if cohort=='loss' else 1
    if statuses[0]!=statuses[primary]:put(diff,[cohort,u,name,i,labels[primary],statuses[primary],statuses[0]])
   if cohort=='loss':
    for h in byHist[u]:
     k=h['canonical'],h['occurrence']
     historical.append(dict(h,observations={l:{'page':pages[j],'case':maps[j].get(k)} if pages[j] is not None else None for j,l in enumerate(labels)},disposition=(maps[0][k][0] if k in maps[0] else 'missing-name')))
  cohortStats[cohort]={'selected':len(rows),'counts':dict(zip(labels,cc)),'paired':pairs}
baselineLoss={(h['url'],h['canonical'],h['occurrence']):h for h in load(census/'historical-losses/rows.json')}
for h in historical:
 back=baselineLoss[h['url'],h['canonical'],h['occurrence']]
 h.update(prior902fe811f=back['current'],originalClassification=back['classification'],originalDetail=back['detail'])
save('cohorts.json',cohortStats);save('historical-292.json',historical);assert len(historical)==292 and len(set(x['url'] for x in historical))==159
oldhist=load(plat/'historical-292.json')
def key(h):return h['url'],h['canonical'],h['occurrence']
assert len(set(key(h) for h in historical))==292
oldidx={(x[0],x[1],x[2]):x for x in oldhist};assert set(oldidx)=={key(h) for h in historical}
for h in historical:
 q=h['observations'];record=q['prior35df']['case'];assert oldidx[key(h)][4]==(record[0] if record else None),(key(h),oldidx[key(h)],record)
with gzip.open(out/'port-configs.json.gz','wt') as f:json.dump(configs,f,separators=(',',':'))
save('local-stable-receipts.json',stablePaths)
with gzip.open(out/'local-stable-provenance.jsonl.gz','wt') as f:
 for u,record in sorted(stableInputRows.items()):put(f,[u,*record])
allgu=set().union(*(set(x) for x in new.values()));coverage=collections.Counter()
with gzip.open(census/'comparison/inventory.jsonl.gz','rt') as inp,gzip.open(out/'noncss-coverage.jsonl.gz','wt') as dest:
 for line in inp:
  u,src,blob,env,reason,prev,*x=json.loads(line);groups=[c for c in new if u in new[c]]
  state='observed' if groups else reason or 'outside this merge guard'
  assert not (groups and reason),(u,reason)
  coverage[state]+=1;put(dest,[u,src,blob,env,state,groups])
assert sum(coverage.values())==31827
prov={'merged':'aa0268b479a6b463da16910b4b743cf38901d962','prior52a':'52a207a13cbd4ee9da7daa0ebfda73a0db9b29f1','prior35df':'35dfb7474ab60c46ba537a98060e2789fb2c4662','wpt':pin,'JSC':'0e0dd7038a7a-6f4649e395e0','columns':labels,'receipts':inproof,'binSha256':{k:sha(f) for k,f in [('merged',run/'pyro'),('prior52a',plat/'integration-a414'),('prior35df',plat/'final')]},'coverage':coverage,'distinctGuardURLs':len(allgu),'uniquePrimaryNonCSS':len(new['main'])+len(new['extra']),'historicalDisposition':collections.Counter(h['disposition'] for h in historical),'historicalOverlapWith52a':len(set(prior35)&set(ref52))}
assert prov['binSha256']['merged']==(run/'binary.sha256').read_text().split()[0] and prov['binSha256']['prior35df']=='e5ddaa6bc18125410917843e328d6680325f99690b202e8e5f573926d876f1a6'
assert prov['binSha256']['prior52a']=='32d537d5963af5db4f39b3d33620c14c2f28d7311f7de622cd7303741f0d656b'
prov.update(localStable={'version':'157.0.8103.80','os':'macOS 27.2.1 arm64','source':'corrected same-pin Chrome census','unmeasuredHere':'local stable CSS receipts unavailable'},publicStable={'id':5478394868006912,'version':'157.0.8103.38','os':'Linux 22.04','revision':'1eecaa0c300058f0a47472f8a4733552f929cc63','unchangedSourceCount':sum(v[2] for v in sources.values())},publicCanary={'id':4907012216061952,'version':'159.0.8143.0','os':'Linux 22.04','revision':pin},jscLibrarySHA256='3deb71f8e7d14daf174e35340a4ff0c1900b0e9f576503adeb5facbb1201f029',finishedGuardReceipts=len(new),namedIdentity='canonical name and one-based occurrence within URL and run')
prov['selectedSourceBlobsChecked']=len(sourcefiles)
prov['referenceReportSHA256']={s:sha(r.parent/'round6-inventory'/f'report-{s}.download') for s in ('stable','canary')}
prov['historicalKeysSHA256']=sha(r/'loss-audit-batch2/canonical-losses.json')
prov['baselineCensusCommit']='fdf757052183b8df60dae474f478d20de16aa436'
prov['priorIntegrationEvidenceCommit']='f80be4736'
save('provenance.json',prov)
