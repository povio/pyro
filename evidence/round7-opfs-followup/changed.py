import json,collections,gzip
from pathlib import Path
w=Path(__file__).parent
by={}
for l in ['base','candidate','chrome158']:
    by[l]={}
    for r in json.load(open(w/(l+'.json')))['results']:
        seen=collections.Counter()
        for n in r['subtests']:
            seen[n['name']]+=1
            by[l][r['url'],n['name'],seen[n['name']]]=[n['status'],n.get('msg')]
rows=[];lost=[]
for key in sorted(set(by['base'])|set(by['candidate'])|set(by['chrome158'])):
    b,h,c=[by[k].get(key) for k in ['base','candidate','chrome158']]
    if b==h:continue
    row=[*key,b,h,c];rows.append(row)
    if b and b[0]=='Pass' and (not h or h[0]!='Pass'):lost.append(row)
for name,data in [('changed-diagnostics.jsonl.gz',rows),('loss-diagnostics.jsonl.gz',lost)]:
    with gzip.open(w/name,'wt') as f:
        for row in data:f.write(json.dumps(row,ensure_ascii=False,separators=(',',':'))+'\n')
print('changed diagnostics',len(rows),'prior passes lost',len(lost))
