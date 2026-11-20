from pathlib import Path
import fcntl, os
import sqlite3,zlib,json,re,subprocess,hashlib
p=Path(__file__).resolve().parent
m=p.parent
d=m.parent
binary=d/'reconciled/pyro-entities'
assert hashlib.sha256(binary.read_bytes()).hexdigest()=='9f2bfe099b2d87b0626040695a0583d460c9f7b3c8a401941761c2d9b6595cb4'
db=sqlite3.connect(f'file:{m}/scores.sqlite?mode=ro',uri=True)
eligible={u for u, in db.execute("select url from result where label='combined2'")}
excluded={}
for u,b in db.execute("select url,raw from result where label in ('base','combined','combined2')"):
 if u not in eligible:continue
 r=json.loads(zlib.decompress(b))
 if r['status'] in ('crash','hang') or re.search('SIG(?:SEGV|ABRT|BUS|ILL)|segmentation fault|process (?:exit|crash)|target closed|browser disconnected',str(r.get('error')),re.I):excluded[u]=r['status']
tasks=json.loads((m/'tasks.json').read_bytes())
losses={r['url'] for r in json.loads((d/'loss-audit-batch2/canonical-losses.json').read_bytes())}
plans=[]
for key in tasks:
 urls=[u for u in json.loads((m/(key+'.json')).read_bytes()) if u in eligible and u not in excluded]
 if urls:plans.append((key,urls))
assert {u for k,urls in plans for u in urls}==eligible-excluded.keys()
(p/'coverage.json').write_text(json.dumps(dict(upstreamMatched=len(eligible),withheld=excluded,selected=len(eligible)-len(excluded)),indent=2))
ordered=sorted(plans,key=lambda kv:-len(losses.intersection(kv[1])))
for key,urls in ordered[int(os.environ.get("CENSUS_SLOT",0))::int(os.environ.get("CENSUS_SLOTS",1))]:
 if os.environ.get("CENSUS_SKIP")==key:continue
 lockdir=p/"locks";lockdir.mkdir(exist_ok=True)
 lock=(lockdir/key).open("a")
 try:fcntl.flock(lock,fcntl.LOCK_EX|fcntl.LOCK_NB)
 except BlockingIOError:continue
 selection=p/'selections'/(key+'.json');selection.write_text(json.dumps(urls))
 out=p/(key+'.json')
 if out.exists():continue
 with (p/(key+'.log')).open('w') as log:
  cmd=['bun','bench/wpt/run.mjs','--kind','testharness','--wpt',str(d.parent/'wptci/wpt-full'),'--server','wptserve','--testdriver','--http2','--generated-globals',str(d.parent/'round6-inventory/MANIFEST-pinned.json'),'--manifest-inventory','--dirs','non-css','--jobs','4','--tests',str(selection),'--bin','lp='+str(binary),'--out',str(out)]
  runner=d/'manifest-runner'
  if key.startswith('legacy-reviewed-'):cmd.append('--measure-reviewed-skips');runner=d/'final-manifest-runner'
  subprocess.run(cmd,cwd=runner,stdout=log,stderr=subprocess.STDOUT,check=True)
 rows=json.loads(out.read_bytes())['results']
 assert sorted(r['url'] for r in rows)==sorted(urls)
 with (p/'completed.jsonl').open('a') as f:print(json.dumps([key,len(rows)]),file=f)
