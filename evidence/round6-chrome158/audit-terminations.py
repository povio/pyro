from pathlib import Path
import json,gzip,re,collections,sqlite3,zlib
p=Path(__file__).resolve().parent;cache=p.parents[1]; m=cache/'round6-noncss-measure/manifest-runs'
q=sqlite3.connect((m/'scores.sqlite').as_uri()+'?mode=ro&immutable=1',uri=True)
labels=[k for k, in q.execute('select distinct label from result')]
seen=collections.defaultdict(list);paths=collections.defaultdict(set);unscoped=[];driver=[];az={}
for label in labels:
 for u,path,b in q.execute('select url,path,raw from result where label=?',(label,)):
  r=json.loads(zlib.decompress(b));err=r.get('error') or '';st=r['status']
  if st in ('hang','crash','panic') or re.search(r'(?:SIG(?:TRAP|SEGV|BUS|ILL|ABRT)|segmentation fault|bus error|browser (?:exited|crashed|died)|process (?:exit|crash))',err,re.I):seen[u].append([label,st,err[:180]])
  paths[path].add(u)
# Match each intact receipt to its contributing rows (including earlier delegated runs).
for path,urls in paths.items():
 f=Path(path);data=json.loads(f.read_bytes())
 if data.get('restarts',0) or data.get('watchdogKills',0):
  unscoped.append(dict(receipt=path,urls=sorted(urls),restarts=data.get('restarts'),watchdog=data.get('watchdogKills'),explicit=[u for u in sorted(urls) if u in seen]))
for lane in ('base','combined','combined2','chrome','chromeWpt'):
 for f in (m/lane).glob('*.log'):
  text=f.read_text(errors='replace')
  if 'error: Protocol error (Page.handleJavaScriptDialog): No dialog is showing' in text or 'error: targetInfo: {' in text:driver.append(str(f.relative_to(m)))
(p/'plans/native-observed.json').write_text(json.dumps(seen,indent=2))
(p/'plans/restart-receipts.json').write_text(json.dumps(unscoped,indent=2))
(p/'plans/driver-only-interruptions.json').write_text(json.dumps(driver,indent=2))
print('labels',labels,'explicit',len(seen),'receipts reviewed',len(paths),'restarts',[(Path(v['receipt']).parent.name,Path(v['receipt']).name,v['restarts'],len(v['urls']),len(v['explicit'])) for v in unscoped],flush=True)
