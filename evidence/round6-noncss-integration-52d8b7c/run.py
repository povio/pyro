from pathlib import Path
import sys,json,hashlib,subprocess,collections,time,select,os,signal
n=Path(__file__).resolve().parent;d=n.parent.parent;cache=d.parent;ref=cache/'round6-reference-158/census';tasks=json.load(open(n/'tasks.json'))
mode=sys.argv[1];root=n/('raw-head' if mode=='new' else 'raw-base'); root.mkdir(exist_ok=True); binary=n/('candidate' if mode=='new' else 'base')

def waitfile(f):
 if f.exists():return
 fd=os.open(f.parent,os.O_RDONLY);q=select.kqueue();q.control([select.kevent(fd,filter=select.KQ_FILTER_VNODE,flags=select.KQ_EV_ADD|select.KQ_EV_CLEAR,fflags=select.KQ_NOTE_WRITE)],0,0)
 while not f.exists():q.control([],1,None)
 os.close(fd)
waitfile(n/('candidate.sha256' if mode=='new' else 'base.sha256'))
sha=lambda f:hashlib.sha256(f.read_bytes()).hexdigest()
print('binary',mode,sha(binary),flush=True)
for t in tasks:
 key=t['key'];sel=n/'selections'/(key+'.json');f=root/(key+'.json')
 if f.exists():continue
 assert sha(sel)==t['sha256']
 cmd=['bun','bench/wpt/run-no-native-retry.mjs','--kind','testharness','--wpt',str(cache/'wptci/wpt-full'),'--server','wptserve','--testdriver','--http2','--generated-globals',str(cache/'round6-inventory/MANIFEST-pinned.json'),'--manifest-inventory','--measure-reviewed-skips','--dirs',t['scope'],'--jobs','4','--tests',str(sel),'--bin','lp='+str(binary),'--out',str(f)]
 start=time.time()
 with (root/(key+'.log')).open('x') as log:
  result=subprocess.run(cmd,cwd=ref/'runner',stdout=log,stderr=subprocess.STDOUT)
 if result.returncode or not f.exists():
  with (root/'failures.jsonl').open('a') as fo:print(json.dumps(dict(key=key,rc=result.returncode)),file=fo)
  continue
 run=json.load(open(f));assert len(run['results'])==t['count'] and {v['url'] for v in run['results']}==set(json.load(open(sel))) and run['wptRevision']=='a421f2fea5eb7f91e0db399e44b0fb0779ee1603'
 with (root/'completed.jsonl').open('a') as fo:print(json.dumps(dict(key=key,count=t['count'],restarts=run['restarts'],watchdogKills=run['watchdogKills'],seconds=round(time.time()-start),receiptSHA256=sha(f))),file=fo)
