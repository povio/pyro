#!/usr/bin/env python3
"""Read-only verifier. Writes JSON only to stdout. Pass a frozen export directory."""
import argparse, collections, functools, gzip, hashlib, itertools, json, pathlib, re, sqlite3, zlib
P=pathlib.Path
args=argparse.ArgumentParser()
args.add_argument('export',type=P)
args.add_argument('--census',type=P,default=P.home()/'Library/Caches/lightningpanda/round6-noncss-measure/manifest-runs/census-902fe811f')
args.add_argument('--final',action='store_true')
args.add_argument('--sample',type=int,default=64)
opt=args.parse_args()
exp=opt.export.resolve();census=opt.census.resolve();m=census.parent;root=m.parent;cache=root.parent
load=lambda p:json.loads(p.read_bytes())
sha=lambda p:hashlib.sha256(p.read_bytes()).hexdigest()
def jsonl(p):
 with gzip.open(p,'rt') as f:
  for line in f:yield json.loads(line)
def ro(p):return sqlite3.connect(p.resolve().as_uri()+'?mode=ro&immutable=1',uri=True)
cnt=collections.Counter;default=collections.defaultdict
passstatus=lambda x:x in ('PASS','Pass')
checks={};notices=[]
checksums=exp/'SHA256SUMS'
if checksums.exists():
 for row in checksums.read_text().splitlines():
  h,f=row.split('  ',1);assert sha(exp/f)==h,(f,'checksum mismatch')
 checks['checksums']=sum(1 for _ in checksums.read_text().splitlines())
base=ro(m/'scores.sqlite');public=ro(root/'reference-ledger.sqlite3')
# Independent inventory accounting, including exact membership and metadata of every URL.
inventory={x['url'].lstrip('/'):x for x in jsonl(cache/'round6-inventory/pinned-inventory.jsonl.gz')}
assert len(inventory)==31827
official=load(m/'final-official-inventory.json')
manifest_env={r['url']:r['environment'] for r in official['eligible']+official['inventoryOnly']}
seen0={r['url'] for r in official['eligible']};omits={r['url']:r.get('reason','inventory-only') for r in official['inventoryOnly']}
assert seen0.isdisjoint(omits) and seen0 | omits.keys()==inventory.keys()
held={r['url'] for r in load(m/'deferred-scope.json')};wasm=set(load(m/'combined2-withheld-runtime.json')['urls'])
wire=set(load(m/'transport-inventory-only.json')['urls'])
assert len(omits)==469 and len(held)==304 and len(wasm)==538 and len(wire)==20
assert wire<=omits.keys() and held.isdisjoint(wasm) and held<=seen0 and wasm<=seen0
cov=load(exp/'coverage.json') if (exp/'coverage.json').exists() else load(census/'coverage.json')
prior={u for u, in base.execute("select url from result where label='combined2'")}
priorChrome={u for u, in base.execute("select url from result where label='chromeWpt'")}
assert prior==seen0-held-wasm and prior<=priorChrome
hist={}
for u,b in base.execute("select url,raw from result where label in ('base','combined','combined2')"):
 if u not in prior:continue
 r=json.loads(zlib.decompress(b))
 if r['status'] in {'crash','hang'} or re.search('SIG(?:SEGV|ABRT|BUS|ILL)|segmentation fault|process (?:exit|crash)|target closed|browser disconnected',str(r.get('error')),re.I):hist[u]=r['status']
assert hist==cov['withheld'];sel=prior-hist.keys();assert len(sel)==cov['selected']==30513
reasons={**omits,**dict.fromkeys(held,'previously reviewed outside ordinary execution'),**dict.fromkeys(wasm,'Wasm runtime withheld'),**{u:'historical '+st for u,st in hist.items()},**dict.fromkeys(wire,'raw HTTP/1 transport unsupported')}
assert set(reasons).isdisjoint(sel) and reasons.keys()|sel==inventory.keys()
tasks=load(m/'tasks.json');urlshard={}
for key in tasks:
 plan=load(m/(key+'.json'));assert len(set(plan))==len(plan) and not set(plan).intersection(urlshard)
 urlshard.update(dict.fromkeys(plan,key))
assert urlshard.keys()==seen0-held
summary=load(exp/'summary.json');labels=['combined2','chromeWpt','publicStable','publicCanary'][:len(summary['columns'])-1]
pages={}
for u,env,*values in jsonl(exp/'pages.jsonl.gz'):
 assert u not in pages and u in sel and len(values)==1+len(labels) and env==manifest_env[u],u
 pages[u]=values
if opt.final:
 assert pages.keys()==sel and labels==['combined2','chromeWpt','publicStable','publicCanary']
 assert 'publicStableUnchangedSource' in summary['comparisons']
 assert (exp/'observed-gaps.json').is_file()
assert summary['selected']==len(sel) and summary['notYetReported']==len(sel)-len(pages)
env=default(cnt);visible=set();reported=0
for fields in jsonl(exp/'inventory.jsonl.gz'):
 u,src,blob,envname,why,vals,*rest=fields
 assert u not in visible and (src,blob)==(inventory[u]['source'],inventory[u]['blob']);visible.add(u)
 assert envname==manifest_env[u]
 assert why==reasons.get(u,None if u in pages else 'awaiting measurement'),(u,why,reasons.get(u))
 assert vals==pages.get(u) and (not rest or rest==[inventory[u]['stable_source_unchanged']])
 env[envname][why or 'reported']+=1
assert visible==inventory.keys() and env==load(exp/'environments.json')
# Validate receipts for all reported current pages; select deterministic cross-area raw reference checks.
sample=set(sorted(pages,key=lambda u:hashlib.sha256(u.encode()).digest())[:opt.sample])
for area in ['ai','domparsing','xhr','html','cookies','event-timing','infrastructure','encoding','url','webnn']:
 v=sorted(u for u in pages if u.split('/')[0]==area)
 if v:sample.add(v[len(v)//2])
rawrows=default(dict);restartReceipts={};nhead=0;perShard=default(set)
for u in pages:perShard[urlshard[u]].add(u)
def row_summary(r,pub=False):
 names=r['subtests'];sts=[t[1] if pub else t['status'] for t in names];hs=r['status'] if pub else r['harnessStatus']
 return [r['status'],hs,bool(sts) and all(passstatus(s) for s in sts) and hs=='OK' and (pub or r['status']=='complete')]
for k,us in perShard.items():
 d=load(census/(k+'.json'))
 assert d['wptRevision']=='a421f2fea5eb7f91e0db399e44b0fb0779ee1603' and d['jobs']==4 and d['testdriver'] and d['server']=='wptserve' and d['http2'] and d['manifestInventory']
 assert d['inventoryPlan']['viewport']=={'width':800,'height':600,'deviceScaleFactor':1} and d['inventoryPlan']['manifestSha256']==official['manifestSha256']
 rr=d['results'];actual={r['url'] for r in rr}
 assert len(rr)==len(actual) and actual==us==set(load(census/'selections'/(k+'.json')))==set(load(m/(k+'.json')))&sel
 if d['restarts'] or d['watchdogKills']:restartReceipts[k]={'restarts':d['restarts'],'watchdogKills':d['watchdogKills']}
 for r in rr:
  u=r['url'];assert (r['source'],r['blob'],r['environment'])==(inventory[u]['source'],inventory[u]['blob'],manifest_env[u]) and pages[u][0]==row_summary(r)
  nhead+=sum(x['status']=='Pass' for x in r['subtests'])
  if u in sample:rawrows[u][0]=r
assert nhead==summary['counts']['namedPass']
# Prefer original local browser receipts over trusting a database row; independently check that both agree.
for label,idx in [('combined2',1),('chromeWpt',2)]:
 byfile=default(dict)
 for u in sample:
  path,b=base.execute('select path,raw from result where label=? and url=?',(label,u)).fetchone()
  byfile[path][u]=json.loads(zlib.decompress(b))
 for path,want in byfile.items():
  rows=load(P(path))['results'];got={r['url']:r for r in rows if r['url'] in want}
  assert got==want,(label,path,'database differs from original receipt')
  for u,r in got.items():assert pages[u][idx]==row_summary(r);rawrows[u][idx]=r
for idx,label in enumerate(labels,1):
 if idx<3:continue
 table='reference' if label=='publicStable' else 'canary'
 for u in sample:
  rec=public.execute(f'select payload from {table} where url=?',('/'+u,)).fetchone()
  r=json.loads(zlib.decompress(rec[0])) if rec else None
  assert pages[u][idx]==(row_summary(r,True) if r else None)
  if r:rawrows[u][idx]=r
def origcases(r,pub):
 return cnt(map(tuple,r['subtests'])) if pub else cnt((s['name'],s['status']) for s in r['subtests'])
raw_names={u:{i:origcases(r,i>=3) for i,r in cols.items()} for u,cols in rawrows.items()}
total=cnt();pairs=default(cnt);areas=default(cnt);reportedGroups=set();numNamedRows=0;canonicalRewrites=0
for u,vals in pages.items():
 total.update(pages=1,fullyPassing=int(vals[0][2]));total['page_'+vals[0][0]]+=1
 a=areas[u.split('/')[0]];a.update(pages=1,lpFull=int(vals[0][2]),chromeFull=int(vals[2][2]))
 for i,label in enumerate(labels,1):
  ref=vals[i];c=pairs[label];h=vals[0]
  if ref is None:c['missingPage']+=1;continue
  for tar in ([c,pairs['publicStableUnchangedSource']] if label=='publicStable' and inventory[u]['stable_source_unchanged'] else [c]):
   if i>=3:tar['paired']+=1
   tar.update(onlyHeadFull=int(h[2] and not ref[2]),onlyRefFull=int(ref[2] and not h[2]),refFull=int(ref[2]))
for u,group in itertools.groupby(jsonl(exp/'named.jsonl.gz'),lambda a:a[0]):
 assert u in pages and u not in reportedGroups;reportedGroups.add(u)
 previous=None;present=cnt();passed=cnt();counts=default(cnt)
 for _,name,n,*cols in group:
  assert previous is None or (name,n)>previous
  previous=name,n;assert len(cols)==len(labels)+1
  flags=[];numNamedRows+=1
  for idx,col in enumerate(cols):
   flags.append(col is not None and passstatus(col[0]))
   if col is None:continue
   assert pages[u][idx] is not None
   status,original=col
   if idx>=3:assert name==original,(u,'public name changed')
   counts[name][idx]+=1;assert counts[name][idx]==n,(u,name,n,idx)
   present[idx]+=1;passed[idx]+=int(flags[-1]);canonicalRewrites+=int(name!=original)
   if u in sample:
    original_key=original,status
    assert raw_names[u][idx][original_key]>0,(u,idx,original_key)
    raw_names[u][idx][original_key]-=1
  if flags[0]:total['namedPass']+=1
  area=areas[u.split('/')[0]]
  area.update(lpOnly=int(flags[0] and not flags[2]),chromeOnly=int(flags[2] and not flags[0]))
  for i,label in enumerate(labels,1):
   if pages[u][i] is None:continue
   for c in ([pairs[label],pairs['publicStableUnchangedSource']] if label=='publicStable' and inventory[u]['stable_source_unchanged'] else [pairs[label]]):
    c.update(onlyHead=int(flags[0] and not flags[i]),onlyRef=int(flags[i] and not flags[0]),both=int(flags[0] and flags[i]))
 for idx,v in enumerate(pages[u]):
  if v:assert v[2]==(bool(present[idx]) and present[idx]==passed[idx] and v[1]=='OK' and (idx>=3 or v[0]=='complete'))
for u in sample:assert all(not (+v) for v in raw_names[u].values()),(u,'unexported raw names')
assert total==cnt(summary['counts'])
if (census/'original-runner-shards.json').exists():
 originals=set(load(census/'original-runner-shards.json'))
 assert not originals.intersection(restartReceipts),('original retry-capable receipts require native-retry review',originals.intersection(restartReceipts))
 checks['originalRunnerShardsAudited']=len(originals.intersection(perShard))
 zero={u for u,v in pages.items() if u not in reportedGroups}
 assert all(not any(r and r[2] for r in pages[u]) for u in zero),'zero-named page reported fully passing'
for label,c in summary['comparisons'].items():assert pairs[label]==cnt(c),(label,pairs[label],c)
assert {k:cnt(v) for k,v in load(exp/'areas.json').items()}==areas
gaps=exp/'observed-gaps.json'
if gaps.exists():
 g=load(gaps);compatible=g['urls']==len(pages) and g['chromeOnly']==pairs['chromeWpt']['onlyRef']
 if not compatible:
  if opt.final:raise ValueError('gap diagnostics from different snapshot')
  notices.append('gap diagnostics are from a different cutoff')
 else:
  assert sum(r['count'] for r in g['groups'])==g['chromeOnly']
  assert all(sum(n for _,n in g['byArea'].get(k,[]))==v['chromeOnly'] for k,v in areas.items())
checks.update(manifest=len(visible),selected=len(sel),withheld=len(reasons),observed=len(pages),namedRows=numNamedRows,currentPasses=nhead,columns=len(labels)+1,sourceAndPageReceiptBlobsChecked=len(pages),sampledUrlsFullOriginalNamedParity=len(sample),encodedNamesRewrittenAcrossColumns=canonicalRewrites,rawReceiptRestarts=restartReceipts,canonicalHistoricalWithheld=hist)
print(json.dumps({'checks':checks,'notices':notices},indent=2))
