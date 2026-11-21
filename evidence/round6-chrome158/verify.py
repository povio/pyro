"""Verify reference inventory, canonical names, all selected receipt rows and optional paired totals offline."""
import pathlib,gzip,json,itertools,collections,hashlib,sys
from canonical_ports import canonical_name,DEFAULTS
P=pathlib.Path; d=P(sys.argv[1]) if len(sys.argv)>1 else P(__file__).resolve().parent
def G(p):return json.loads(p.read_bytes())
def lines(p):
 with gzip.open(p,'rt') as f:
  for line in f:yield json.loads(line)
def digest(v):return hashlib.sha256(json.dumps(v,separators=(',',':'),ensure_ascii=True).encode()).hexdigest()
if (d/'SHA256SUMS').exists():
 for line in (d/'SHA256SUMS').read_text().splitlines():
  h,n=line.split('  ',1);assert hashlib.sha256((d/n).read_bytes()).hexdigest()==h,n
coverage=G(d/'coverage.json');ident=G(d/'reference-identity.json');provenance=G(d/'receipt-provenance.json');configs=G(d/'server-port-maps.json')
assert ident['version']=='158.0.8127.37' and ident['channel']=='Google Chrome stable'
schema={};globalStats=collections.Counter();observed=set();byShard=collections.defaultdict(set)
for u,src,blob,env,scope,groups,status,reasons,key,page in lines(d/'official-pages.jsonl.gz'):
 assert u not in schema and src and len(blob)==40
 schema[u]=(src,blob,env,scope,page)
 globalStats[scope]+=1
 if page:
  assert status=='observed' and not reasons and key in provenance
  observed.add(u);byShard[key].add(u)
 else:assert key is None and ((status=='explicitly withheld')==bool(reasons))
assert dict(globalStats)=={'CSS':8162,'non-CSS':31827} and len(observed)==coverage['observed']
if coverage['stage']=='final':assert coverage['selected']==len(observed)
hashes={};sortedRaw={}
for k,meta in provenance.items():
 f=d/'receipts'/(k+'.json.gz')
 with gzip.open(f,'rt') as inp:data=json.load(inp)
 rows=data['results'];assert len(rows)==len(byShard[k]) and {x['url'] for x in rows}==byShard[k] and set(data['plan'])==byShard[k]
 assert data['wptRevision']==ident['wpt'] and data['inventoryPlan']['manifestSha256']==ident['manifestSha256'] and data['inventoryPlan']['viewport']=={'width':800,'height':600,'deviceScaleFactor':1} and data['jobs']==4
 assert data['runnerProvenance']['rawOriginalSHA256']==meta['rawSHA256'] and data['runnerProvenance']['browserSHA256']==ident['executableSHA256']
 portmap={}
 for role,ports in configs[meta['configurationSHA256']].items():
  if role not in DEFAULTS:continue
  proto='https' if role=='h2' else role.split('-')[0]
  for port,target in zip(ports,DEFAULTS[role]):portmap[proto,port]=target
 for r in rows:
  u=r['url'];src,blob,env,scope,page=schema[u];assert r['source']==src and r['blob']==blob and r['environment']==env and page[:2]==[r['status'],r['harnessStatus']]
  n=[];occur=collections.Counter()
  for t in r['subtests']:
   first=t['name'];canon=canonical_name(first,portmap);occur[canon]+=1
   n.append([canon,occur[canon],t['status'],first])
  passed=sum(it[2]=='Pass' for it in n)
  assert page[2:]==[bool(n) and r['status']=='complete' and r['harnessStatus']=='OK' and passed==len(n),len(n),passed]
  if n:hashes[u]=digest(n)
  sortedRaw[u]=digest(sorted(n))
for u,group in itertools.groupby(lines(d/'reference-names.jsonl.gz'),key=lambda x:x[0]):
 assert hashes.pop(u)==digest([v[1:] for v in group]),u
assert not hashes
fv=d/'fadc-comparison.json'
if fv.exists():
 comp=G(fv)['comparison'];total=collections.defaultdict(collections.Counter);seen={}
 for g,u,h,c in lines(d/'fadc-pages.jsonl.gz'):
  assert (g,u) not in seen;seen[g,u]=[h,c]
  if c is None:total[g]['missingChromePages']+=1
  else:
   assert schema[u][4]==c
   total[g].update(paired=1,lpFull=int(h[2]),chromeFull=int(c[2]))
 for g,u,n,idx,h,c in lines(d/'fadc-names.jsonl.gz'):
  if seen[g,u][1] is None:assert c is None;continue
  hp=h is not None and h[0]=='Pass';cp=c is not None and c[0]=='Pass'
  total[g].update(lpPass=int(hp),chromePass=int(cp),lpOnly=int(hp and not cp),chromeOnly=int(cp and not hp),both=int(cp and hp),chromeNameAbsent=int(h is not None and c is None),lpNameAbsent=int(c is not None and h is None))
 assert all(total[g]==collections.Counter(v) for g,v in comp.items())
 for (g,u),gr in itertools.groupby(lines(d/'fadc-names.jsonl.gz'),key=lambda t:(t[0],t[1])):
  actual=[[n,i,c[0],c[1]] for _,_,n,i,h,c in gr if c is not None]
  assert digest(sorted(actual))==sortedRaw.get(u,digest([])),(g,u,'reference occurrence mismatch')
 h=G(d/'historical-292.json');assert len(h)==292 and len({(r['url'],r['canonical'],r['occurrence']) for r in h})==292
if (d/'cbdeb-comparison.json').exists():
 ct=collections.defaultdict(collections.Counter);views={}
 for u,h,c,k in lines(d/'cbdeb-pages.jsonl.gz'):
  assert u in observed and u not in views;views[u]=(h,c)
  scope=schema[u][3].lower()
  if h is None:ct[scope]['missingLpPages']+=1
  else:ct[scope].update(paired=1,lpFull=int(h[2]),chromeFull=int(c[2]))
 assert set(views)==observed
 for u,gr in itertools.groupby(lines(d/'cbdeb-names.jsonl.gz'),key=lambda t:t[0]):
  hpg,cpg=views[u];assert hpg is not None
  actual=[];scope=schema[u][3].lower()
  for _,name,idx,a,b in gr:
   hp=a is not None and a[0]=='Pass';cp=b is not None and b[0]=='Pass'
   if b is not None:actual.append([name,idx,*b])
   ct[scope].update(lpPass=int(hp),chromePass=int(cp),lpOnly=int(hp and not cp),chromeOnly=int(cp and not hp),both=int(hp and cp),missingLpName=int(a is None and b is not None),missingChromeName=int(b is None and a is not None))
  assert digest(sorted(actual))==sortedRaw[u],u
 assert ct=={k:collections.Counter(v) for k,v in G(d/'cbdeb-comparison.json')['scopes'].items()}
if (d/'cross-machine-inventory.jsonl.gz').exists():
 cross=collections.defaultdict(collections.Counter);seen=set();studio={}
 for u,src,blob,env,coh,spg,lpg,key,swhy,lwhy in lines(d/'studio-css-pages.jsonl.gz'):
  assert u not in studio and schema[u][:3]==(src,blob,env) and schema[u][4]==lpg
  studio[u]=spg
 for u,source,blob,env,scope,tag,l,r in lines(d/'cross-machine-inventory.jsonl.gz'):
  assert u not in seen and schema[u][:4]==(source,blob,env,scope);seen.add(u)
  assert l['page']==schema[u][4] and r['page']==studio.get(u)
  cross[scope][tag]+=1
 assert len(studio)==8162 and len(seen)==39989 and dict(cross)==G(d/'cross-machine-summary.json')['scope']
 sc=G(d/'studio-compatibility.json')['paired'];names=collections.Counter()
 for u,n,i,a,b in lines(d/'studio-css-names.jsonl.gz'):
  if not (studio[u] and schema[u][4]):continue
  ap=a=='Pass';bp=b=='Pass';names.update(commonPass=int(ap and bp),onlyStudioPass=int(ap and not bp),onlyLocalPass=int(bp and not ap),studioPass=int(ap),localPass=int(bp))
 for k,v in names.items():assert sc[k]==v,(k,sc[k],v)
addons=set()
for prefix in ('storage-only-addendum','reflection-only-addendum'):
 if not (d/(prefix+'.json')).exists():continue
 result=G(d/(prefix+'.json'));urls=set(result['urls']);assert not urls&addons and not (urls&observed)
 assert all(schema[u][4] is None for u in urls);addons.update(urls)
 with gzip.open(d/(prefix+'-receipts.json.gz'),'rt') as f:original=json.load(f)
 for label,doc in original.items():
  assert {r['url'] for r in doc['results']}==urls and doc['jobs']==4 and doc['restarts']==doc['watchdogKills']==0
  actual=collections.Counter(t['status'] for r in doc['results'] for t in r['subtests'])
  assert actual==collections.Counter(result['counts'][label])
if coverage['stage']=='final':assert len(addons)==12
print('Verified',len(observed),'Chrome 158 page observations against original named outcomes in',len(provenance),'receipts;',len(schema),'unique official variants.')
