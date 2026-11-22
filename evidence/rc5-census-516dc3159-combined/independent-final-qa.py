from pathlib import Path
import hashlib,json,gzip,collections,itertools,zipfile,subprocess,plistlib
C=Path.home()/'Library/Caches/lightningpanda/round7-local';D=C/'export-release-516dc3159';P=D/'combined-publication/evidence/rc5-census-516dc3159-combined';E=C/'census-release-516dc3159';O=D/'studio-transfer/raw';cmp=P/'comparison';asym=P/'asymmetric'
def sha(p):
 with p.open('rb') as f:return hashlib.file_digest(f,'sha256').hexdigest()
def J(p):return json.loads(p.read_bytes())
def lines(p):
 with gzip.open(p,'rt') as f:yield from map(json.loads,f)
def pack(x):return json.dumps(x,ensure_ascii=True,separators=(',',':')).encode()
def digest(items):return hashlib.sha256(pack(sorted(items))).digest()
def cell(p):
 a=p['subtests'];n=sum(x['status']=='Pass' for x in a)
 return [p['status'],p.get('harnessStatus'),bool(a and n==len(a) and p['status']=='complete' and p.get('harnessStatus')=='OK'),n,len(a)]
for line in (P/'SHA256SUMS').read_text().splitlines():
 h,n=line.split('  ',1);assert sha(P/n)==h
tasks=J(E/'plans-v2/tasks.json'); start='516dc3159ff4602bd3a11e98607c262c0ffc65c4';b='9b2454c277a52452150d59b33c64c7d46f0206781c0068f5a6b0dd0631ffd87e'
assert sha(E/'shipped/pyro')==J(O/'machine-provenance.json')['executableSHA256']==b
with zipfile.ZipFile(E/'shipped/archive-arm64.zip') as z:assert hashlib.sha256(z.read('pyro')).hexdigest()==b
m=J(E/'machine-provenance.json');o=J(O/'machine-provenance.json');assert (m['head'],m['workflowRunID'],m['artifactID'])==(o['tagCommit'],o['workflowRun'],o['artifactID'])==(start,44998826328,14745758172)
assert J(P/'even-jsc-runtime.json')['frameworkUUID'] in o['jscLoaded'][0] and J(P/'even-jsc-runtime.json')['frameworkBundleVersion']==o['jscBundle']=='22625.2.8.11.9';assert 'fork: false' in m['binaryVersion'] and 'fork: false' in o['runtimeVersion']
pr=D/'studio-transfer/ports';proofs={r['key']:r for r in J(pr/'role-port-proof/manifest.json')};assert len(proofs)==160
egen={r['key']:r for r in map(json.loads,(E/'raw/completed.jsonl').read_text().splitlines())};ogen={r['key']:r for r in map(json.loads,(O/'completed.jsonl').read_text().splitlines())}
counts=[collections.Counter(),collections.Counter()];names={};pages={};ords=set()
for t in tasks:
 i=t['index'];k=t['key'];odd=i%2;assert i not in ords;ords.add(i); assert t['owner']==('studio' if odd else 'local')
 sel=E/'plans-v2/selections'/(k+'.json');r=ogen[k] if odd else egen[k];assert r['index']==i and t['sha256']==sha(sel)==r.get('selection',r.get('selectionSHA256'))
 p=O/k/'result.json' if odd else E/'raw'/(k+'.json');assert sha(p)==r.get('rawSHA256',r.get('receiptSHA256'))
 raw=J(p);selurls=J(sel);assert set(selurls)==set(raw['plan'])=={x['url'] for x in raw['results']} and len(selurls)==len(raw['results'])==t['count']
 assert raw['jobs']==4 and raw['wptRevision']=='a421f2fea5eb7f91e0db399e44b0fb0779ee1603' and raw['inventoryPlan']['manifestSha256']=='34a67512a84d43630f7adbecebd4378df5f410446591015cc6a20a3034e02819'
 if odd:
  proof=proofs[k]; assert proof['index']==i and proof['attempt']==1
  log=(O/k/'run.log').read_text();assert ('(log '+proof['originalServerLog']+')') in log
  for name in ('config.json','serve.log'):assert sha(pr/proof[name]['path'])==proof[name]['sha256']
  a=J(pr/proof['config.json']['path']);assert ('wptserve on http://web-platform.test:'+str(a['ports']['http'][0])) in log
  norm=J(D/'studio-transfer/normalized-machine/verified-ports'/(k+'.json'));assert a['ports']==norm['proof']['ports'] and norm['proof']['sha256']==proof['config.json']['sha256']
 c=counts[odd];c['shards']+=1;c['restarts']+=raw['restarts'];c['watchdogKills']+=raw['watchdogKills']
 for pg in raw['results']:
  u=pg['url'];assert u not in pages; x=cell(pg);pages[u]=x;c['urls']+=1;c[pg['status']]+=1;c['strict']+=x[2];c['passed']+=x[3];c['observed']+=x[4]
  n=collections.Counter();items=[]
  for s in pg['subtests']:
   n[s['name']]+=1;items.append([s['name'],n[s['name']],s['status']])
  names[u]=digest(items)
assert ords==set(range(320)) and len(pages)==30179
hist=C/'census-c3372ad21-odds/final-combined'
pa={u:c for u,_,c,*_ in lines(hist/'pages.jsonl.gz')}
refnames={}
for u,grp in itertools.groupby(lines(hist/'named.jsonl.gz'),key=lambda r:r[0]):
 refnames[u]=digest([[r[1],r[2],r[4]] for r in grp if r[4] is not None])
tot=collections.Counter();direction={}
for ty in ('pages','names'):
 fp=cmp/('pages.jsonl.gz' if ty=='pages' else 'named.jsonl.gz')
 streams={engine:iter(lines(asym/(engine+'-only-'+ty+'.jsonl.gz'))) for engine in ('chrome','lp')}
 if ty=='names':
  confirmed={engine:iter(lines(asym/(engine+'-only-confirmed-names.jsonl.gz'))) for engine in streams}
 seen=set()
 for u,grp in itertools.groupby(lines(fp),key=lambda r:r[0]):
  assert u not in seen;seen.add(u);members=[];cmembers=[]
  for r in grp:
   if ty=='pages':
    a,b=r[1:3];assert pages[u]==a and pa[u]==b;ap,bp=a[2],b[2];tot['chromeObserved']+=b[4];tot['chromePassed']+=b[3];tot['chromeStrict']+=b[2]
   else:
    a,b=r[3:5];ap=a is not None and a[0]=='Pass';bp=b is not None and b[0]=='Pass'
    if b is not None:cmembers.append([r[1],r[2],b])
    if a is not None:members.append([a[1],r[2],a[0]])
   if ap!=bp:
    engine='lp' if ap else 'chrome';assert next(streams[engine])==r
    if ty=='names':assert next(confirmed[engine])==r
    tot[engine+'Only_'+ty]+=1
  if ty=='names':assert names[u]==digest(members) and refnames.get(u,digest([]))==digest(cmembers)
 assert all(next(s,None) is None for s in streams.values())
 if ty=='names':
  assert all(next(s,None) is None for s in confirmed.values()) and all(names[u]==digest([]) for u in pages.keys()-seen)
 else:assert seen==pages.keys()
assert not any(lines(asym/'unresolved-mappings.jsonl.gz')) and not J(asym/'port-mapping-audit.json')['unresolvedConfigs']
assert (tot['chromePassed'],tot['chromeObserved'],tot['chromeStrict'])==(1754948,1802021,24385)
result=dict(verified=True,source=start,binarySHA256=m['binarySHA256'],workflow=44998826328,artifact=14745758172,systemJSCUUID=J(P/'even-jsc-runtime.json')['frameworkUUID'],even=counts[0],odd=counts[1],uniqueURLs=len(pages),uniqueOrdinals=len(ords),authenticatedStudioPortConfigs=len(proofs),unresolvedMappings=0,comparison=tot,checked='All raw LP page cells and original name/occurrence/status sets, archived signed Chrome names and page cells, directional named and page stream rows plus confirmed-only, full artifact checksums and release executable identity.')
(D/'independent-final-qa.json').write_text(json.dumps(result,indent=2)+'\n');print(json.dumps(result))
