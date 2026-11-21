from pathlib import Path
import json,hashlib,gzip,collections
p=Path(__file__).resolve().parent
cache=p.parents[2]
ref=cache/'round6-reference-158/census'
guard=p.parents[1]/'merged-aa0268/integration-52d8b7c'
def load(f):return json.loads(f.read_bytes())
def sha(f):return hashlib.sha256(f.read_bytes()).hexdigest()
def save(f,x):f.write_text(json.dumps(x,indent=1)+'\n')
if __name__=='__main__':
 eligible=set(load(ref/'plans/shared-noncss.urls.json'))
 assert len(eligible)==30359
 holds=collections.defaultdict(list)
 for r in load(ref/'export-final/process-events.json'):
  for row in r['explicitRows']:holds[row['url']].append(['Chrome158',row['status']])
  assert not r['unassignedNativeExitPossible']
 for u in load(ref/'export-final/studio-native-markers.json')['urls']:holds[u].append(['Studio','native marker'])
 for rs in (ref/'raw',p.parent/'census-cbdeb1193/raw',p.parents[1]/'merged-aa0268/integration-284548/raw-base',p.parents[1]/'merged-aa0268/integration-284548/raw-head'):
  for line in (rs/'completed.jsonl').read_text().splitlines():
   r=json.loads(line)
   if r['restarts'] or r['watchdogKills'] or set(r.get('statuses',{}))&{'crash','hang','panic'}:
    receipt=load(rs/(r['key']+'.json'))
    bad=[a for a in receipt['results'] if a['status'] in ('crash','panic','hang')]
    ambiguous=r['watchdogKills'] or len(bad)!=r['restarts']
    for a in (receipt['results'] if ambiguous else bad):holds[a['url']].append([rs.parent.name,r['key'],a['status']])
 kept=eligible-set(holds)
 reuse={}
 for t in load(guard/'tasks.json'):
  for u in load(guard/'selections'/(t['key']+'.json')):
   if u in kept:assert u not in reuse;reuse[u]=t['key']
 todo=kept-reuse.keys();tasks=[]
 for r in load(ref/'plans/tasks.json'):
  k=r['key'];src=ref/'selections'/(k+'.json');assert sha(src)==r['sha256'];urls=[u for u in load(src) if u in todo]
  if urls:
   f=p/'selections'/(k+'.json');save(f,urls);tasks.append(dict(key=k,scope='non-css',count=len(urls),sha256=sha(f),referenceSelectionSHA256=r['sha256']))
 assert sum(t['count'] for t in tasks)==len(todo)
 save(p/'tasks.json',tasks);save(p/'guard-membership.json',reuse);save(p/'new-holds.json',{u:holds[u] for u in sorted(eligible-kept)})
 print({'historicalEligible':len(eligible),'newHeld':len(eligible-kept),'sameHeadGuard':len(reuse),'additionalUrls':len(todo),'shards':len(tasks)})
