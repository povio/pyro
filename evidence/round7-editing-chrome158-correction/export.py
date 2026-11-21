"""Read and compare affected static LP/Chrome runs with the signed reference and public calibration."""
from pathlib import Path
import collections,gzip,hashlib,json,subprocess,sys
p=Path(__file__).resolve().parent;c=p.parents[1];ed=c/'round7-editing';ref=c/'round6-reference-158/census';oldexe=Path('/Applications/Google Chrome.app/Contents/MacOS/Google Chrome');newexe=c/'round6-reference-158/Google Chrome.app/Contents/MacOS/Google Chrome';G=lambda f:json.loads(f.read_bytes());sha=lambda f:hashlib.sha256(f.read_bytes()).hexdigest()
def save(n,o):(p/n).write_text(json.dumps(o,indent=1,sort_keys=True)+'\n')
def emit(f,x):print(json.dumps(x,separators=(',',':')),file=f)
def rows(f):
 with gzip.open(f,'rt') as s:yield from map(json.loads,s)
urls=set(G(p/'urls.json'));assert len(urls)==47
identity=G(ref/'reference-identity.json');assert sha(newexe)==identity['executableSHA256']
def ver(exe):return subprocess.check_output(['/usr/libexec/PlistBuddy','-c','Print CFBundleShortVersionString',str(exe.parents[1]/'Info.plist')],text=True).strip()
assert ver(newexe)=='158.0.8127.37' and ver(oldexe)=='157.0.8103.80'
idcols=['lpBase','lpCandidate','historicalSystem157Static','signed158Static','signed158Census']
def raw(f,kind):
 r=G(f);assert r['wptRevision']==identity['wpt'] and r['jobs']==1 and r['server']=='static' and not r['testdriver'] and not r['reuseContext'] and not r.get('http2')
 assert r['restarts']==0;assert r.get('watchdogKills',0)==0
 if kind=='new':assert Path(r['bin']).resolve()==ref/'launch-158'
 if kind=='old':assert Path(r['bin']).resolve()==c/'round6-chrome-stable-profile/chrome-wpt-profile.py'
 out={}
 for v in r['results']:
  n=collections.Counter();nn={}
  for s in v['subtests']:
   name=s['name'];n[name]+=1;nn[name,n[name]]=[s['status'],name]
  npass=sum(x[0]=='Pass' for x in nn.values());full=bool(nn and v['status']=='complete' and v['harnessStatus'] in (0,'OK') and npass==len(nn))
  assert v['url'] not in out;out[v['url']]=([v['status'],v['harnessStatus'],full,npass,len(nn)],nn)
 assert set(r['plan'])==set(out);return out
fresh=raw(p/'signed158-static.json','new');assert set(fresh)==urls
cen={}
for u,source,blob,env,scope,guard,status,omit,sh,pg in rows(ref/'export-final/official-pages.jsonl.gz'):
 if u in urls:
  assert status=='observed' and pg;cen[u]=([*pg[:3],pg[4],pg[3]],{})
for u,n,i,status,orig in rows(ref/'export-final/reference-names.jsonl.gz'):
 if u in urls:
  assert (n,i) not in cen[u][1];cen[u][1][n,i]=[status,orig]
assert cen.keys()==urls
files={f.parent.relative_to(ed).as_posix():f for f in ed.rglob('chrome.json') if G(f).get('bin')==str(c/'round6-chrome-stable-profile/chrome-wpt-profile.py')}
cmp={};pro={};observed=set();unionLegacy=collections.defaultdict(list)
with gzip.open(p/'page-sets.jsonl.gz','wt') as pgout,gzip.open(p/'named-sets.jsonl.gz','wt') as nmout:
 for name,f in sorted(files.items()):
  legacy=raw(f,'old')
  # Future runs may add variants; these require their own verified receipt.
  if not legacy.keys()<=urls:raise ValueError((name,legacy.keys()-urls))
  base=raw(f.parent/'base.json','lp');head=raw(f.parent/'head.json','lp')
  assert legacy.keys()==base.keys()==head.keys();observed.update(legacy)
  pro[name]={k:sha(x) for k,x in [('base',f.parent/'base.json'),('head',f.parent/'head.json'),('historicalSystem157',f)]}
  groups=[base,head,legacy,fresh,cen];stat=[collections.Counter() for _ in groups];pairs=collections.defaultdict(collections.Counter)
  for u in sorted(legacy):
   vals=[m[u] for m in groups];emit(pgout,[name,u,*[v[0] for v in vals]])
   for i,(page,ns) in enumerate(vals):
    st=stat[i];st['pages']+=1;st['full']+=page[2];st['names']+=page[4];st['passes']+=page[3];st['page/'+page[0]]+=1
    assert len(ns)==page[4] and sum(s[0]=='Pass' for s in ns.values())==page[3]
   unionLegacy[u].append((name,vals[2]))
   for key in sorted(set.union(*[set(v[1]) for v in vals])):
    answers=[v[1].get(key) for v in vals];emit(nmout,[name,u,*key,*answers]);yes=[bool(x and x[0]=='Pass') for x in answers]
    for a,b in [(0,1),(2,3),(3,4),(1,3),(1,4)]:
     grp=pairs[idcols[a]+'To'+idcols[b]];grp['gained']+=yes[b] and not yes[a];grp['lost']+=yes[a] and not yes[b];grp['addedNames']+=answers[a] is None and answers[b] is not None;grp['missingNames']+=answers[a] is not None and answers[b] is None
    if (not yes[0]) and yes[1]:pairs['lpGainsAlsoPassingOnChrome']['signed158Static']+=yes[3];pairs['lpGainsAlsoPassingOnChrome']['historical157Static']+=yes[2]
  cmp[name]={'columns':dict(zip(idcols,stat)),'comparisons':pairs}
assert observed==urls
def keysets(table):
 return {(u,k) for u,(page,ns) in table.items() for k,v in ns.items() if v[0]=='Pass'}
def summarize(table):
 return dict(pages=len(table),fullPages=sum(v[0][2] for v in table.values()),passes=len(keysets(table)),names=sum(v[0][4] for v in table.values()))
legacyFirst={u:obs[0][1] for u,obs in unionLegacy.items()}
inconsistent={u:[g for g,v in obs] for u,obs in unionLegacy.items() if any(v!=obs[0][1] for _,v in obs)}
tot={l:summarize(t) for l,t in [('signed158Static',fresh),('signed158Census',cen)]}
if not inconsistent:
 tot['historicalSystem157Static']=summarize(legacyFirst);tot['historical157ToSigned158Static']=dict(gained=len(keysets(fresh)-keysets(legacyFirst)),lost=len(keysets(legacyFirst)-keysets(fresh)))
tot['signed158StaticToOfficial158Census']=dict(gained=len(keysets(cen)-keysets(fresh)),lost=len(keysets(fresh)-keysets(cen)))
save('receipt-mapping.json',pro);save('comparisons.json',cmp);save('summary.json',{'columns':idcols,'distinct':tot,'historicalDifferencesAcrossReceipts':inconsistent,'distinctURLs':len(urls),'receiptAliases':len(files),'historicalReferenceVersion':ver(oldexe),'verifiedStaticVersion':ver(newexe),'historicalExecutableSHA256':sha(oldexe),'signedExecutableSHA256':sha(newexe),'historicalLauncherSHA256':sha(c/'round6-chrome-stable-profile/chrome-wpt-profile.py'),'signedLauncherSHA256':sha(ref/'launch-158'),'newReceiptSHA256':sha(p/'signed158-static.json'),'newRunnerSHA256':sha(p/'runner-src/bench/wpt/run-no-native-static.mjs'),'wpt':identity['wpt'],'manifest':identity['manifestSha256'],'censusEvidence':'ee24e07f6b15b399d6e8e2157c763809341b6e44','newMeasurement':dict(jobs=1,server='static',testdriver=False,restarts=0,watchdogKills=0)})
print(tot,'inconsistent',inconsistent)
