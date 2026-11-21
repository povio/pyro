from pathlib import Path
from collections import Counter
import json,gzip,sys,re
p=Path(__file__).resolve().parent
def read(n):
 d=json.loads((p/f'{n}.json').read_text())
 pages={r['url']:r for r in d['results']};names={}
 for r in pages.values():
  c=Counter()
  for t in r['subtests']:
   c[t['name']]+=1
   names[r['url'],t['name'],c[t['name']]]=t['status']
 strict={r['url'] for r in pages.values() if r.get('harnessStatus')=='OK' and r['status']=='complete' and r['subtests'] and all(t['status']=='Pass' for t in r['subtests'])}
 return pages,names,strict
def pack(labels,stem):
 data=list(map(read,labels));keys=sorted(set().union(*(d[1].keys() for d in data)));urls=sorted(set().union(*(d[0].keys() for d in data)))
 with gzip.open(p/(stem+'-named.jsonl.gz'),'wt') as out:
  for k in keys:out.write(json.dumps([*k,*[d[1].get(k) for d in data]],ensure_ascii=True)+'\n')
 with gzip.open(p/(stem+'-pages.jsonl.gz'),'wt') as out:
  for url in urls:out.write(json.dumps([url,*[([d[0][url]['status'],d[0][url].get('harnessStatus'),url in d[2]] if url in d[0] else None) for d in data]])+'\n')
 sets=[{k for k,v in d[1].items() if v=='Pass'} for d in data]
 return {'labels':labels,'urlCounts':[len(d[0]) for d in data],'names':[len(d[1]) for d in data],'passCounts':list(map(len,sets)),'strictPages':[len(d[2]) for d in data],'pageOutcomes':[dict(Counter((r['status']+'/'+str(r.get('harnessStatus'))) for r in d[0].values())) for d in data],'gained':len(sets[1]-sets[0]),'lost':len(sets[0]-sets[1]),'added':len(data[1][1].keys()-data[0][1].keys()),'missing':len(data[0][1].keys()-data[1][1].keys()),'newlyPassingMatchChrome':len((sets[1]-sets[0]) & sets[2]) if len(sets)>2 else None}

report={'focus':pack(['base','head','chrome158'],'focus'),'guard':pack(['guard-base','guard-head'],'guard')}
(p/'summary.json').write_text(json.dumps(report,indent=2)+'\n')
print(json.dumps(report))
