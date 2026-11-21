from pathlib import Path
import json,gzip,subprocess,hashlib,collections,sys
n=Path(__file__).resolve().parent
d=n.parent
c=d.parent
p=d/'merged-aa0268/integration-b62d664'
v=d/'video-frame-round6'
runner=c/'round6-reference-158/census/runner'
sys.path.insert(0,str(d/'manifest-runs/census-902fe811f'))
from wpt_ports import config,index
def read(f):
 return json.loads(gzip.decompress(f.read_bytes()) if f.suffix=='.gz' else f.read_bytes())
def save(f,data):f.write_text(json.dumps(data,indent=1)+'\n')
def sha(f):return hashlib.sha256(f.read_bytes()).hexdigest()
inventory={r[0]:r for l in gzip.decompress(subprocess.check_output(['git','show','3bcc56d4:evidence/round6-single-head-158/inventory.jsonl.gz'],cwd=n/'src')).splitlines() if (r:=json.loads(l))}
inputs=[(t['key'],p/'raw-head'/(t['key']+'.json'),t['scope']) for t in read(p/'tasks.json')]
inputs += [('video-'+k,v/('vetted-'+k+'.json'),'non-css') for k in ('focus','guard')]
inputs += [('css-values',n/'src/docs/validation/css-values-a7f8d09e/timing-fonts/candidate.json.gz','css')]
inputs += [('studio-closeout',n/'studio-focus-reference.json','css')]
summary={}; omissions={}; allurls=set()
for key,ref,scope in sorted(inputs,key=lambda t: {'studio-closeout':-3,'css-values':-2,'production-00':1}.get(t[0],0)):
 prev=read(ref); planned=prev['plan']; assert len(set(planned))==len(planned)
 urls=[u for u in planned if (inventory[u][5] or (key=='production-00' and u=='wasm/serialization/module/serialization-via-notifications-api.any.worker.html')) and not u.startswith('websockets/back-forward-cache-with-open-websocket-connection-and-close-it-in-pagehide')]
 omissions[key]={u:inventory[u][7] for u in planned if u not in urls}
 allurls.update(urls)
 sel=n/(key+'.selection.json'); save(sel,urls)
 out=n/(key+'.json'); log=n/(key+'.log')
 cmd=['bun','bench/wpt/run-no-native-retry.mjs','--kind','testharness','--wpt',str(c/'wptci/wpt-full'),'--server','wptserve','--generated-globals',str(c/'round6-inventory/MANIFEST-pinned.json'),'--manifest-inventory','--measure-reviewed-skips','--dirs',scope,'--jobs',str(prev['jobs']),'--tests',str(sel),'--bin','lp='+str(n/'pyro'),'--out',str(out)]
 if prev['testdriver']:cmd.extend(['--testdriver'])
 if prev.get('http2'):cmd.append('--http2')
 if sys.argv[1]=='plan':continue
 if not out.exists():
  with log.open('x') as f:subprocess.run(cmd,cwd=runner,stdout=f,stderr=subprocess.STDOUT,check=True)
 cur=read(out)
 assert cur['wptRevision']==prev['wptRevision']=='a421f2fea5eb7f91e0db399e44b0fb0779ee1603'
 assert cur['inventoryPlan']['manifestSha256']==prev['inventoryPlan']['manifestSha256']
 assert set(urls)==set(cur['plan'])=={r['url'] for r in cur['results']}
 def sets(report,mp):
  names={};pages=set();outer={}
  for r in report['results']:
   u=r['url']
   if u not in urls:continue
   rows,collisions=index([(t['name'],t['status']) for t in r['subtests']],mp)
   assert not collisions
   for (name,occ),st in rows.items():
    if u.endswith('/protocol.https.html'):
     for (scheme,port),target in mp.items():
      if scheme=='https' and target==8443:name=name.replace('https://web-platform.test:%s'+str(port)+'/', 'https://web-platform.test:%s8443/')
    names[(u,name,occ)]=st
   outer[u]=[r['status'],r['harnessStatus']]
   if list(outer[u])==['complete','OK'] and rows and all(x[0]=='Pass' for x in rows.values()):pages.add(u)
  return names,pages,outer
 a,ap,ao=sets(prev,config(str(ref.with_suffix('.log')))[0] if key!='css-values' else {})
 b,bp,bo=sets(cur,config(str(log))[0])
 gains=[];losses=[];missing=[]
 with gzip.open(n/(key+'.named.jsonl.gz'),'wt') as f:
  for k in sorted(a.keys()|b.keys()):
   l=a.get(k);r=b.get(k);print(json.dumps([*k,l,r]),file=f)
   if (l and l[0]=='Pass') and not(r and r[0]=='Pass'):losses.append(k)
   if (r and r[0]=='Pass') and not(l and l[0]=='Pass'):gains.append(k)
   if l is None or r is None:missing.append(k)
 save(n/(key+'.pages.json'),[[u,ao[u],bo[u],u in ap,u in bp] for u in urls])
 summary[key]=dict(count=len(urls),jobs=cur['jobs'],old=sum(x[0]=='Pass' for x in a.values()),new=sum(x[0]=='Pass' for x in b.values()),gains=len(gains),losses=len(losses),nameDifferences=len(missing),oldFull=len(ap),newFull=len(bp),fullGains=sorted(bp-ap),fullLosses=sorted(ap-bp),restarts=cur['restarts'],watchdog=cur['watchdogKills'],statuses=dict(collections.Counter(r['status'] for r in cur['results'])),referenceHash=sha(ref),receiptHash=sha(out))
 save(n/'summary.json',summary)
 print(key,len(urls),len(gains),len(losses),flush=True)
save(n/'omissions.json',omissions)
print('distinct URLs',len(allurls),flush=True)
