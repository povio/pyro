"""Passively compare immutable Studio observations and local same-version CSS without pooling their counts."""
import collections,gzip,hashlib,itertools,json,pathlib,sys,subprocess
P=pathlib.Path;d=P(__file__).resolve().parent;src=d/'studio-source';out=P(sys.argv[1]);out.mkdir(parents=True,exist_ok=True)
G=lambda f:json.loads(f.read_bytes())
def readgz(f):return json.loads(gzip.open(f,'rb').read())
def dump(x):return json.dumps(x,separators=(',',':'),ensure_ascii=True)
def get(rev,name):
 path='docs/validation/studio-css-e4ca26/'+name;f=src/name
 oid=subprocess.check_output(['git','rev-parse',rev+':'+path],text=True).strip()
 data=subprocess.check_output(['git','cat-file','blob',oid])
 assert hashlib.sha1(b'blob '+str(len(data)).encode()+b'\0'+data).hexdigest()==oid
 if f.exists():assert f.read_bytes()==data
 else:f.write_bytes(data)
 return f,{'gitCommit':rev,'gitPath':path,'gitBlob':oid,'sha256':hashlib.sha256(data).hexdigest()}
base='fd629a078f697139cf140adbb2a315e90037bb25';supp='8e84143126a3335af41906350060477c69751616';files={}
f,files['main']=get(base,'outcomes-stable.json.gz');main=readgz(f)
f,files['supplement']=get(supp,'chrome-wpt-cert-supplement.json.gz');additional=readgz(f)
f,files['coverage']=get(base,'coverage.json.gz');cov=readgz(f)
f,files['mainReadme']=get(base,'README.md');assert '158.0.8127.37' in f.read_text() and 'a421f2fea5eb7f91e0db399e44b0fb0779ee1603' in f.read_text()
f,files['supplementReadme']=get(supp,'color-gap-controls.md');assert 'certificate' in f.read_text()
assert set(main)==set(cov['matched']) and len(main)==8082
sup={r['url']:[r['status'],r['harnessStatus'],[[s['name'],s['status']] for s in r['subtests']]] for r in additional};assert len(sup)==41 and not sup.keys()&main.keys()
assert set(cov['not discovered by project runner'])==set(sup)
records={**main,**sup};omit=set(cov['out of scope: crash or security target']);assert len(omit)==39
official={r['url']:r for l in gzip.open(d/'plans/official-inventory.jsonl.gz','rt') if (r:=json.loads(l))['scope']=='CSS'};assert set(records)|omit==official.keys() and not omit&set(records)
meta=G(d/'plans/tasks.json');local={};rawkeys={}
for t in meta:
 if t['scope']!='css':continue
 f=d/'raw'/(t['key']+'.json')
 if not f.exists():continue
 for r in G(f)['results']:local[r['url']]=r;rawkeys[r['url']]=t['key']
def named(items):
 seen=collections.Counter();n={}
 for name,status in items:
  seen[name]+=1;n[name,seen[name]]=status
 return n
def strict(outer,hs,vals):return outer=='complete' and hs in (0,'OK') and bool(vals) and all(s=='Pass' for s in vals.values())
all155=collections.Counter();stats=collections.Counter();symbol=collections.defaultdict(collections.Counter);deltas=collections.defaultdict(list);missingBoth=[];unmatchedStudio=[];scope=collections.Counter()
with gzip.open(out/'studio-css-pages.jsonl.gz','wt',compresslevel=4) as pg,gzip.open(out/'studio-css-names.jsonl.gz','wt',compresslevel=4) as nm:
 for u,o in sorted(official.items()):
  s=records.get(u);l=local.get(u);ss='original 8082' if u in main else 'certified 41' if u in sup else 'excluded'
  if s:
   so,sh,tests=s;sn=named(tests);sp={k for k,v in sn.items() if v=='Pass'};sf=strict(so,sh,sn);srow=[so,sh,sf,len(sn),len(sp)];all155.update(observed=1,full=int(sf),named=len(sn),namedPass=len(sp))
  else:sn={};sp=set();srow=None
  if l:
   ln=named([(t['name'],t['status']) for t in l['subtests']]);pl={k for k,v in ln.items() if v=='Pass'};lf=strict(l['status'],l['harnessStatus'],ln);lrow=[l['status'],l['harnessStatus'],lf,len(ln),len(pl)]
  else:ln={};pl=set();lrow=None
  if s and l:
   c=symbol[ss];v=dict(matched=1,studioNamed=len(sn),localNamed=len(ln),studioPass=len(sp),localPass=len(pl),commonPass=len(sp&pl),onlyStudioPass=len(sp-pl),onlyLocalPass=len(pl-sp),studioFull=int(sf),localFull=int(lf),bothFull=int(lf and sf),onlyStudioFull=int(sf and not lf),onlyLocalFull=int(lf and not sf),namesMissingLocal=len(sn.keys()-ln.keys()),namesMissingStudio=len(ln.keys()-sn.keys()))
   stats.update(v);c.update(v)
   if sl:=len(sp-pl)+len(pl-sp):deltas['named'].append([u,len(sp-pl),len(pl-sp)])
   if sf!=lf:deltas['page'].append([u,sf,lf])
  elif s:unmatchedStudio.append(u)
  elif not l:missingBoth.append(u)
  tag='both' if s and l else 'studio-only' if s else 'local-only' if l else 'neither';scope[tag]+=1
  print(dump([u,o['source'],o['blob'],o['environment'],ss,srow,lrow,rawkeys.get(u),None if s else 'Studio: crash or security target, excluded',None if l else o['withheld']]),file=pg)
  for k in sorted(sn.keys()|ln.keys()):print(dump([u,*k,sn.get(k),ln.get(k)]),file=nm)
z={'wpt':'a421f2fea5eb7f91e0db399e44b0fb0779ee1603','version':'158.0.8127.37','studioSources':files,'studioInventory':8162,'studioRecorded':all155,'studioOnly':len(unmatchedStudio),'missingBoth':len(missingBoth),'crossMachineCoverage':scope,'paired':stats,'byStudioCohort':symbol,'strictPageDifferences':deltas['page'],'namedDifferencesLargest':sorted(deltas['named'],key=lambda x:-sum(x[1:]))[:18],'originalDriverSettings':'Studio: headless CDP/disposable profile, 16 original Chrome shards; 4 used disabled extensions/component background pages. Supplemental 41 used the WPT leaf SPKI. Exact browser flags, per-shard concurrency, Playwright version and raw server port configurations are not fully specified by those receipts. Local: Playwright 1.65.1, isolated WPT stable preset, SPKI, native GPU, H2, 4 jobs. Use original subtest names and one-based occurrences across machines; do not infer an absent server port mapping.'}
(out/'studio-compatibility.json').write_text(json.dumps(z,indent=2)+'\n')
print(json.dumps({k:z[k] for k in ('studioRecorded','crossMachineCoverage','paired','byStudioCohort')},indent=1))
