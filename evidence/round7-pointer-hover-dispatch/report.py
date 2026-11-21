import gzip,json,sys
from collections import Counter
from pathlib import Path
here=Path(__file__).resolve().parent
def read(n):
 p=here/(n+'.json')
 if p.exists():return json.loads(p.read_text())
 return json.loads(gzip.decompress((here/(n+'.json.gz')).read_bytes()))
def write(n,data):
 (here/n).write_bytes(gzip.compress((''.join(json.dumps(x,ensure_ascii=True)+'\n' for x in data)).encode(),mtime=0))
runs=[read(n) for n in ('parent-wpt','candidate-wpt','chrome158')]
summaries={}
for key in ('focus','guards'):
 urls=read(key); tables=[]; named={}; pages={}
 for i,run in enumerate(runs):
  table={r['url']:r for r in run['results'] if r['url'] in urls}
  assert list(sorted(table))==sorted(urls)
  statuses={}; fully=set()
  for u in urls:
   row=table[u]; count=Counter()
   for t in row['subtests']:
    count[t['name']]+=1; statuses[u,t['name'],count[t['name']]]=t['status']
   if row['status']=='complete' and row['harnessStatus']=='OK' and statuses and row['subtests'] and all(t['status']=='Pass' for t in row['subtests']):fully.add(u)
   pages.setdefault(u,[None]*3)[i]=[row['status'],row['harnessStatus'],u in fully,row.get('ms')]
  for (u,n,occ),status in statuses.items(): named.setdefault((u,n,occ),[None]*3)[i]=status
  tables.append(({n for n,status in statuses.items() if status=='Pass'},set(statuses),fully))
 parent,candidate,chrome=tables
 lost=parent[0]-candidate[0];gained=candidate[0]-parent[0]
 summaries[key]={'urls':len(urls),'passCounts':[len(t[0]) for t in tables],'nameCounts':[len(t[1]) for t in tables],'fullPages':[len(t[2]) for t in tables],
   'gains':len(gained),'losses':len(lost),'gainsChromePass':len(gained&chrome[0]),'missingNames':len(parent[1]-candidate[1]),'addedNames':len(candidate[1]-parent[1]),'pageOutcomes':[dict(Counter(r[i][0]+'/'+str(r[i][1]) for r in pages.values())) for i in range(3)]}
 write(key+'-names.jsonl.gz',[[*k,*v] for k,v in sorted(named.items())])
 write(key+'-pages.jsonl.gz',[[k,*v] for k,v in sorted(pages.items())])
(here/'summary.json').write_text(json.dumps(summaries,indent=2)+'\n')
print(json.dumps(summaries))
