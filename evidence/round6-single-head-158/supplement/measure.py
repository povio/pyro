from pathlib import Path
import collections,hashlib,json,subprocess,time,sys
q=Path(__file__).resolve().parent;c=q.parent;m=c.parent;d=m.parent;cache=d.parent;ref=cache/'round6-reference-158/census';raw=q/'raw';binary=d/'merged-aa0268/batch/visible'
sha=lambda f:hashlib.sha256(f.read_bytes()).hexdigest();assert sha(binary)=='81d12af223643b8d5c1903fd528a36a5f2ff3040855f998d7bc5e546805b93b2'
for t in json.load(open(q/'tasks.json')):
 k=t['key'];f=raw/(k+'.json');lock=raw/(k+'.claim');sel=q/'selections'/(k+'.json')
 if f.exists() or (raw/(k+'.failed.json')).exists():continue
 try:lock.mkdir()
 except FileExistsError:continue
 try:
  assert sha(sel)==t['sha256'];cmd=['bun','bench/wpt/run-no-native-retry.mjs','--kind','testharness','--wpt',str(cache/'wptci/wpt-full'),'--server','wptserve','--testdriver','--http2','--generated-globals',str(cache/'round6-inventory/MANIFEST-pinned.json'),'--manifest-inventory','--measure-reviewed-skips','--dirs','css','--jobs','4','--tests',str(sel),'--bin','lp='+str(binary),'--out',str(f)]
  start=time.time()
  with (raw/(k+'.log')).open('x') as log:rc=subprocess.run(cmd,cwd=ref/'runner',stdout=log,stderr=subprocess.STDOUT).returncode
  if rc or not f.exists():
   (raw/(k+'.failed.json')).write_text(json.dumps({'key':k,'rc':rc,'reason':'Preserve partial and inspect before any recovery'}));continue
  x=json.load(open(f));assert x['wptRevision']=='a421f2fea5eb7f91e0db399e44b0fb0779ee1603'
  assert len(x['results'])==t['count'] and {r['url'] for r in x['results']}==set(json.load(open(sel)))
  with (raw/'completed.jsonl').open('a') as out:print(json.dumps(dict(key=k,count=t['count'],restarts=x['restarts'],watchdogKills=x['watchdogKills'],seconds=round(time.time()-start),receiptSHA256=sha(f))),file=out)
 finally:lock.rmdir()
