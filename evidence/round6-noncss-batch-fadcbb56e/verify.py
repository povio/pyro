"""Recompute the packaged comparison offline. Usage: python3 verify.py [directory]."""
import collections,gzip,hashlib,itertools,json,pathlib,sys
from canonical_ports import canonical_name,DEFAULTS
p=pathlib.Path(sys.argv[1]) if len(sys.argv)>1 else pathlib.Path(__file__).resolve().parent
def read(n):return json.loads((p/n).read_bytes())
def stream(n):
 with gzip.open(p/n,'rt') as f:
  for line in f:yield json.loads(line)
def eq(a,b):return collections.Counter(a)==collections.Counter(b)
if (p/'SHA256SUMS').exists():
 for line in (p/'SHA256SUMS').read_text().splitlines():
  h,n=line.split('  ',1);assert hashlib.sha256((p/n).read_bytes()).hexdigest()==h,n
prov=read('provenance.json');groups=read('cohorts.json');labels=prov['columns']
with gzip.open(p/'port-configs.json.gz','rt') as f:portGroups=json.load(f)
portMaps={}
for h,roles in portGroups.items():
 mapping={}
 for role,vals in roles.items():
  if role not in DEFAULTS:continue
  assert len(vals)<=len(DEFAULTS[role])
  for actual,official in zip(vals,DEFAULTS[role]):
   protocol='https' if role=='h2' else role.split('-')[0]
   assert (protocol,actual) not in mapping
   mapping[protocol,actual]=official
 portMaps[h]=mapping
tables={};per=collections.defaultdict(lambda:collections.defaultdict(collections.Counter));counts=collections.defaultdict(lambda:collections.defaultdict(collections.Counter));coverage=set()
for g,u,src,blob,same,*pages in stream('pages.jsonl.gz'):
 assert len(pages)==len(labels) and (g,u) not in tables and blob and src
 tables[g,u]=pages;coverage.add(u);full=pages[0][2]
 for j,(l,v) in enumerate(zip(labels,pages)):
  c=counts[g][l]
  if v is None:c['missing']+=1
  else:c.update(observed=1,fully=int(v[2]),names=v[3],namedPass=v[4]);c['page_'+v[0]]+=1
  if j:
   z=per[g][l]
   if v is None:z['missingReferencePage']+=1
   else:z.update(paired=1,onlyHeadFull=int(full and not v[2]),onlyRefFull=int(v[2] and not full))
assert len(coverage)==prov['distinctGuardURLs']
seen=set();historical=read('historical-292.json')
hist={(r['url'],r['canonical'],r['occurrence']):r for r in historical}
assert len(hist)==292 and len({u for u,_,_ in hist})==159
seenHist=set()
for gu,lines in itertools.groupby(stream('names.jsonl.gz'),lambda v:tuple(v[:2])):
 assert gu in tables and gu not in seen;seen.add(gu);pages=tables[gu];g,u=gu;occ=collections.defaultdict(collections.Counter);np=collections.Counter();nt=collections.Counter();prev=None
 for _,_,n,i,*cols in lines:
  k=n,i;assert (prev is None or prev<k) and len(cols)==len(labels);prev=k
  passes=[v is not None and v[0].upper()=='PASS' for v in cols]
  for j,v in enumerate(cols):
   if v is not None:
    assert pages[j] is not None
    occ[n][j]+=1;assert occ[n][j]==i
    if j>=4:assert v[1]==n
    else:assert canonical_name(v[1],portMaps[pages[j][5]])==n
    nt[j]+=1;np[j]+=int(passes[j])
   if j and pages[j] is not None:
    per[g][labels[j]].update(onlyHead=int(passes[0] and not passes[j]),onlyRef=int(passes[j] and not passes[0]),both=int(passes[0] and passes[j]),headMissingName=int(cols[0] is None and v is not None),referenceMissingName=int(v is None and cols[0] is not None))
  if g=='loss' and (u,n,i) in hist:
   h=hist[u,n,i];seenHist.add((u,n,i))
   for j,l in enumerate(labels):
    o=h['observations'][l];assert o==({'page':pages[j],'case':cols[j]} if pages[j] is not None else None)
 for j,v in enumerate(pages):
  if v is not None:assert v[3:5]==[nt[j],np[j]] and v[2]==(v[1]=='OK' and (v[0]=='complete' if j<4 else True) and bool(nt[j]) and np[j]==nt[j]),(g,u,j)
for gu,pages in tables.items():
 if gu not in seen:assert all(v is None or (v[3:5]==[0,0] and not v[2]) for v in pages)
for key,h in hist.items():
 if key not in seenHist:assert all(v is None or v['case'] is None for v in h['observations'].values())
 assert h['disposition']==(h['observations']['candidate']['case'] or ['missing-name'])[0]
referrer=read('historical-referrer-157.json')
assert referrer==[h for h in historical if h['url'].startswith('referrer-policy/')] and len(referrer)==157
for g,v in groups.items():
 for l in labels:assert eq(counts[g][l],v['counts'][l])
 for l in labels[1:]:assert eq(per[g][l],v['paired'][l]),(g,l)
states=collections.Counter();urls=set()
for u,src,blob,env,state,groupsFor in stream('noncss-coverage.jsonl.gz'):
 assert u not in urls;urls.add(u);states[state]+=1
 assert set(groupsFor)=={group for group in groups if (group,u) in tables}
assert len(urls)==31827 and eq(states,prov['coverage'])
assert eq(collections.Counter(h['disposition'] for h in historical),prov['historicalDisposition'])
variantKeys=set()
for g,u,env,scheme,source,blob in stream('variant-inventory.jsonl.gz'):
 assert (g,u) in tables and (g,u) not in variantKeys and env and scheme in ('http','https') and source and len(blob)==40
 variantKeys.add((g,u))
assert variantKeys==tables.keys()
assert prov['currentAcceptanceReference']['version']=='158.0.8127.37' and prov['currentAcceptanceReference']['observedInThisBundle'] is False
assert all(v['restarts']==0 and v['watchdogKills']==0 for v in prov['receipts'].values())
controls=read('focused-controls.json');rr=controls['referrer354'];ur=set(rr['pages'])
assert len(ur)==354 and not ur.intersection(h['url'] for h in hist.values() if h['url'].startswith('referrer-policy/'))
for k in ['parser-probe','child-probe','audio-probe']:
 raw=gzip.open(p/(k+'.json.gz'),'rb').read()
 assert hashlib.sha256(raw).hexdigest()==controls['normalProbes'][k]['sha256']
 if k!='audio-probe':
  probes=json.loads(raw)['pages'];assert len(probes)==24 and all(v['seen'] and v['observed']==v['expected'] and v['pass'] for v in probes)
print('Verified',len(tables),'cohort URLs,',len(coverage),'distinct URLs,',len(hist),'historical occurrences,',len(urls),'non-CSS inventory records.')
