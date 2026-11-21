from pathlib import Path
import collections,concurrent.futures,hashlib,json,os,select,subprocess,sys,time
from common import *
import threading,datetime
sys.path.insert(0,str(ref))
from port_receipt import snapshot_config
raw=p/'raw'
identity=load(ref/'reference-identity.json')
def record(n,o):
 data=(json.dumps(o,separators=(',',':'))+'\n').encode()
 fd=os.open(raw/n,os.O_WRONLY|os.O_APPEND|os.O_CREAT,0o644)
 try:assert os.write(fd,data)==len(data)
 finally:os.close(fd)
def run(t):
 k=t['key'];receipt=raw/(k+'.json');claim=raw/(k+'.claim');log=raw/(k+'.log')
 if log.exists() or receipt.exists() or (p/'PAUSE_NEW').exists():return
 try:claim.mkdir()
 except FileExistsError:return
 (claim/'start.json').write_text(json.dumps(dict(utc=datetime.datetime.now(datetime.timezone.utc).isoformat(),pid=os.getpid(),jobs=4)))
 sel=p/'selections'/(k+'.json');assert sha(sel)==t['sha256']
 args=['bun','bench/wpt/run-no-native-retry.mjs','--kind','testharness','--wpt',str(cache/'wptci/wpt-full'),'--server','wptserve','--testdriver','--http2','--generated-globals',str(cache/'round6-inventory/MANIFEST-pinned.json'),'--manifest-inventory','--measure-reviewed-skips','--dirs','non-css','--jobs','4','--tests',str(sel),'--bin','lp='+str(binary),'--out',str(receipt)]
 start=time.monotonic()
 with log.open('x') as out:rc=subprocess.run(args,cwd=ref/'runner',stdout=out,stderr=subprocess.STDOUT).returncode
 if rc or not receipt.exists():
  record('failures.jsonl',dict(key=k,returncode=rc));return
 obj=load(receipt);rows=obj['results'];assert len(rows)==t['count'] and {r['url'] for r in rows}==set(load(sel))
 assert obj['wptRevision']==identity['wpt'] and obj['inventoryPlan']['manifestSha256']==identity['manifestSha256']
 assert obj['jobs']==4 and obj['testdriver'] and obj['http2'] and not obj['reuseContext'] and obj['inventoryPlan']['viewport']==dict(width=800,height=600,deviceScaleFactor=1)
 snapshot_config(log,p/'verified-ports'/(k+'.json'))
 record('completed.jsonl',dict(key=k,count=len(rows),receiptSHA256=sha(receipt),statuses=collections.Counter(r['status'] for r in rows),restarts=obj['restarts'],watchdogKills=obj['watchdogKills'],seconds=round(time.monotonic()-start)))
 (claim/'start.json').unlink();claim.rmdir()
if __name__=='__main__':
 assert sha(binary)==digest and (qa/'source').read_text().splitlines()[0]==head
 assert sha(ref/'runner/bench/wpt/run-no-native-retry.mjs')==identity['noNativeReplayRunnerSHA256']
 slots=p/'slots';tasks=iter(load(p/'tasks.json'));active=set();exhausted=False;changed=threading.Event()
 with concurrent.futures.ThreadPoolExecutor(max_workers=16) as pool:
  while active or not exhausted:
   for future in list(active):
    if future.done():future.result();active.remove(future)
   limit=int(slots.read_text().strip());assert 0<=limit<=16
   while len(active)<limit and not exhausted:
    t=next(tasks,None)
    if t is None:exhausted=True;break
    future=pool.submit(run,t);active.add(future);future.add_done_callback(lambda f:changed.set())
   if active or not exhausted:changed.wait(5);changed.clear()
