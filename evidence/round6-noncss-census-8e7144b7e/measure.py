from pathlib import Path
import collections,concurrent.futures,hashlib,json,os,select,subprocess,sys,time
from plan import p,ref,guard,cache,load,sha
sys.path.insert(0,str(ref))
from port_receipt import snapshot_config
raw=p/'raw'
identity=load(ref/'reference-identity.json')
def records(f):return [json.loads(l) for l in f.read_text().splitlines()] if f.exists() else []
def guards_finished():
 keys={t['key'] for t in load(guard/'tasks.json')}
 for root in [guard/'raw-base',guard/'raw-head']:
  if (root/'failures.jsonl').exists():raise RuntimeError('Review incomplete guard receipts')
  if {r['key'] for r in records(root/'completed.jsonl')}!=keys:return False
 return True
def wait_guards():
 q=select.kqueue();fds={}
 while True:
  for d in [guard,guard/'raw-base',guard/'raw-head']:
   for f in [d,d/'completed.jsonl']:
    if f in fds or not f.exists():continue
    fd=os.open(f,os.O_RDONLY);fds[f]=fd
    q.control([select.kevent(fd,filter=select.KQ_FILTER_VNODE,flags=select.KQ_EV_ADD|select.KQ_EV_CLEAR,fflags=select.KQ_NOTE_WRITE)],0,0)
  if guards_finished():break
  q.control([],4,None)
 for fd in fds.values():os.close(fd)
 q.close()
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
 claim.rmdir()
if __name__=='__main__':
 print('Waiting for paired guard completion; no duplicate browser workload.',flush=True)
 wait_guards()
 binary=guard/'candidate'
 assert sha(binary)==(guard/'candidate.sha256').read_text().split()[0]
 assert (guard/'commit.txt').read_text().strip()=='8e7144b7e7de68be3b5b345138abe2c1dcc6d232'
 assert sha(ref/'runner/bench/wpt/run-no-native-retry.mjs')==identity['noNativeReplayRunnerSHA256']
 print('Starting remaining selections',sha(binary),flush=True)
 with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:
  list(pool.map(run,load(p/'tasks.json')))
