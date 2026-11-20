import json,math,gzip,zlib,sqlite3,collections,pathlib,hashlib
p=pathlib.Path(__file__).resolve().parent;cache=p.parents[1];root=cache/'round6-noncss-measure/manifest-runs';d=p/'selections';d.mkdir(exist_ok=True)
L=lambda f:json.loads(f.read_bytes()); eligible=set(L(p/'plans/shared-noncss.urls.json'))|set(L(p/'plans/css-guard.urls.json'));priority=set(L(p/'plans/priority.urls.json'))
q=sqlite3.connect((root/'scores.sqlite').as_uri()+'?mode=ro&immutable=1',uri=True);cost={}
for u,raw in q.execute("select url,raw from result where label='chromeWpt'"):
 if u in eligible:cost[u]=max(50,min(63000,json.loads(zlib.decompress(raw)).get('ms') or 1000))
seen=set();tasks=[]
def add(group,scope,urls,nmax):
 us=set(urls)&eligible-seen;seen.update(us)
 assert all(u.startswith('css/')==(scope=='css') for u in us)
 if not us:return
 n=math.ceil(len(us)/nmax);bins=[[] for _ in range(n)];total=[0]*n
 for u in sorted(us,key=lambda u:(-cost.get(u,1000),u)):
  i=min((i for i in range(n) if len(bins[i])<nmax),key=lambda i:(total[i],len(bins[i]),i));bins[i].append(u);total[i]+=cost.get(u,1000)
 for i,arr in enumerate(bins):
  k=f'{group}-{i:03d}';f=d/(k+'.json');f.write_text(json.dumps(sorted(arr),indent=1)+'\n');tasks.append({'key':k,'scope':scope,'count':len(arr),'estimatedBrowserMs':total[i],'sha256':hashlib.sha256(f.read_bytes()).hexdigest()})
add('preflight','non-css',L(cache/'round6-chrome-stable-profile/urls.json'),10)
add('guard-history','non-css',L(cache/'round6-noncss-measure/platform-batch/loss-urls.json'),55)
add('guard-css','css',[u for u in priority if u.startswith('css/')],128)
add('guard-main','non-css',[u for u in priority if not u.startswith('css/')],96)
add('ordinary','non-css',eligible-seen,96)
assert seen==eligible and sum(t['count'] for t in tasks)==len(eligible)
(p/'plans/tasks.json').write_text(json.dumps(tasks,indent=1)+'\n')
print('batches',len(tasks),collections.Counter(t['key'].rsplit('-',1)[0] for t in tasks),flush=True)
