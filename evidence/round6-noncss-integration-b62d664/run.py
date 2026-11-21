"""Measure each frozen shard once, serially, using the verified no-native-retry runner."""
from pathlib import Path
import hashlib,json,os,select,subprocess,sys,time
p=Path(__file__).resolve().parent;c=p.parents[2];runner=c/'round6-reference-158/census/runner'
tasks=json.loads((p/'tasks.json').read_bytes());sha=lambda f:hashlib.sha256(f.read_bytes()).hexdigest()
def waitfile(f):
 if f.exists():return
 q=select.kqueue();fd=os.open(f.parent,os.O_RDONLY)
 try:
  q.control([select.kevent(fd,filter=select.KQ_FILTER_VNODE,flags=select.KQ_EV_ADD|select.KQ_EV_CLEAR,fflags=select.KQ_NOTE_WRITE)],0,0)
  while not f.exists():q.control([],1,None)
 finally:os.close(fd)
for label in ['base','head']:
 binary=p/label;waitfile(p/(label+'.sha256'))
 assert sha(binary)==(p/(label+'.sha256')).read_text().split()[0]
 root=p/('raw-'+label);root.mkdir(exist_ok=True)
 for t in tasks:
  sel=p/'selections'/(t['key']+'.json');assert sha(sel)==t['sha256'];f=root/(t['key']+'.json')
  cmd=['bun','bench/wpt/run-no-native-retry.mjs','--kind','testharness','--wpt',str(c/'wptci/wpt-full'),'--server','wptserve','--testdriver','--http2','--generated-globals',str(c/'round6-inventory/MANIFEST-pinned.json'),'--manifest-inventory','--measure-reviewed-skips','--dirs',t['scope'],'--jobs',str(t['jobs']),'--tests',str(sel),'--bin',label+'='+str(binary),'--out',str(f)]
  with f.with_suffix('.log').open('x') as log:subprocess.run(cmd,cwd=runner,stdout=log,stderr=subprocess.STDOUT,check=True)
  raw=json.loads(f.read_bytes());assert raw['restarts']==raw['watchdogKills']==0 and len(raw['results'])==t['count'] and {r['url'] for r in raw['results']}==set(json.loads(sel.read_bytes()))
  with (root/'completed.jsonl').open('a') as out:print(json.dumps(dict(key=t['key'],count=t['count'],sha256=sha(f))),file=out)
 print(label,'complete',flush=True)
