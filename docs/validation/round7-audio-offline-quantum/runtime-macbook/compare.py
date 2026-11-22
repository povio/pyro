import json
from pathlib import Path
data={s:json.loads(Path(s+'.json').read_text()) for s in ('base','candidate','reference')}
ref=data['reference']
for key in ref:
 if key=='pcm':
  for s,d in data.items():
   rows=d[key]
   print(s,'PCM',[(r['quantum'],r.get('actualQuantum'),sum(c['bad'] for c in r['channels'])) for r in rows])
   assert [r['quantum'] for r in rows]==[128,13,441,33001]
   for r in rows:
    assert len(r['channels'])==2 and all(c['length']==r['quantum']+829 and c['bad']==0 for c in r['channels'])
    if s!='base':assert r['actualQuantum']==r['quantum']
  continue
 def statuses(s):
  item=data[s][key]
  assert item['status']==0,item
  return {r[0]:r[1] for r in item['tests']}
 a,b,c=map(statuses,data)
 assert a.keys()==b.keys()==c.keys() and b==c and all(v==0 for v in c.values())
 print(key,len(c),sum(v==0 for v in a.values()),len(b),'gains',sum(a[k]!=0 and b[k]==0 for k in a))
