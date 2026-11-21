"""Record every frozen-head observation separately, including heterogeneous transport and worker counts."""
from export import *
def main(out):
 f305=d/'combined-305840f45/pyro';base='305840f45afbc4f55bfa24f303c1e858d30e4c1d';expected='058c0ac124f412b2e91960d7ed5ebfb343341e90149fa38cf5004a2c6687dc09'
 assert sha(f305)==expected
 selected={u:(pg,{(n,i):(s,original) for n,i,s,original in names}) for u,pg,names in rows(out/'candidate-profile-overlap.jsonl.gz')}
 archives={f:('frozen',x[-1]) for f,x in load(p/'frozen-archives.json').items()}
 for section in ['css','edit','guards']:
  f=qa/('head-'+section+'.json');archives[str(f.relative_to(cache))]=('smoke',sha(f))
 profiles=collections.defaultdict(set);four_job_keys=[];four_job_urls=set()
 receipts={};urlsets=collections.defaultdict(set);counts=collections.defaultdict(collections.Counter)
 census=load(p/'smoke-overlaps.json')
 with gzip.open(out/'archive-pages.jsonl.gz','wt') as pp,gzip.open(out/'archive-named.jsonl.gz','wt') as nn:
  for key,(lab,checksum) in sorted(archives.items()):
   f=cache/key;doc=load(f);assert sha(f)==checksum and doc['bin']==str(binary if lab=='smoke' else f305)
   profile={k:doc.get(k) for k in ['jobs','server','testdriver','dedicatedWorkers','generatedGlobals','http2','reuseContext','manifestInventory']};profile['viewport']=doc.get('inventoryPlan',{}).get('viewport')
   proof={};m={}
   if doc['server']=='wptserve' and doc['results']:
    sp=p/'archive-ports'/(key.replace('/','_'))
    if f.parent==qa:sp=qa/'ports'/f.name
    m,proof=snapshot_config(f.with_suffix('.log'),sp)
   receipts[key]=dict(sha256=checksum,head=head if lab=='smoke' else base,binarySHA256=digest if lab=='smoke' else expected,profile=profile,ports=proof.get('ports'),configSHA256=proof.get('sha256'),logSHA256=sha(f.with_suffix('.log')) if f.with_suffix('.log').exists() else None)
   exact_four=(lab=='frozen' and profile['jobs']==4 and profile['server']=='wptserve' and profile['http2'] and profile['testdriver'] and profile['manifestInventory'] and profile['generatedGlobals'] and profile['dedicatedWorkers'] and not profile['reuseContext'] and profile['viewport']==dict(width=800,height=600,deviceScaleFactor=1))
   if exact_four:four_job_keys.append(key)
   for r in doc['results']:
    u=r['url']
    if lab=='frozen' and u.startswith('css/'):continue
    if exact_four:assert u not in four_job_urls;four_job_urls.add(u)
    if lab=='frozen':profiles[profile['server']+'/jobs='+str(profile['jobs'])+'/http2='+str(bool(profile['http2']))].add(u)
    urlsets[lab].add(u);oldpg,oldnames=named(r,m);newpg,newnames=selected.get(u,(None,{}))
    emit(pp,[key,u,oldpg,newpg])
    if newpg:page_diff(counts[key],newpg,oldpg)
    for n,i in sorted(oldnames.keys()|newnames.keys()):
     a=newnames.get((n,i));b=oldnames.get((n,i));emit(nn,[key,u,n,i,b,a])
     if newpg:diff(counts[key],a,b)
 exact=collections.Counter()
 for k in four_job_keys:exact.update(counts.get(k,{}))
 save(out/'frozen-four-job-comparison.json',dict(profile='same four-job wptserve H2 testdriver manifest fresh-context viewport profile',sourceURLs=len(four_job_urls),matchedURLs=len(four_job_urls&selected.keys()),sourceReceipts=four_job_keys,round7First_frozen305Second=exact,allFrozenProfiles={k:len(v) for k,v in profiles.items()}))
 save(out/'archive-provenance.json',receipts);save(out/'archive-comparisons.json',dict(distinctSourceURLs={k:len(v) for k,v in urlsets.items()},matchedDistinctURLs={k:len(v&selected.keys()) for k,v in urlsets.items()},byReceipt=counts))
if __name__=='__main__':
 import shutil,subprocess
 out=Path(sys.argv[1]);main(out)
 capacity=p/'capacity-evidence'
 if capacity.exists():
  dest=out/'capacity';dest.mkdir(exist_ok=True)
  for file in sorted(capacity.iterdir()):
   if file.is_file() and file.suffix in ['.md','.json']:
    assert file.stat().st_size<4*1024*1024 and b'/Users/' not in file.read_bytes()
    shutil.copy2(file,dest/file.name)
 if (p/'supplement-attempt-audit.json').exists():shutil.copy2(p/'supplement-attempt-audit.json',out/'supplement-attempt-audit.json')
 shutil.copy2(p/'changes.py',out/'changes.py')
 subprocess.run([sys.executable,str(out/'changes.py'),str(out)],check=True)
 shutil.copy2(p/'verify_archives.py',out/'verify_archives.py')
 subprocess.run([sys.executable,str(out/'verify_archives.py'),str(out)],check=True)
