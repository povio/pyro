"""Rank observed Chrome-only names; matching error text is diagnostic, not causal attribution."""
import collections,json,re,sqlite3,sys,zlib
from pathlib import Path
p=Path(__file__).resolve().parent;root=p.parent
sys.path.insert(0,str(p))
from wpt_ports import endpoint_map,index
db=sqlite3.connect(f'file:{root}/scores.sqlite?mode=ro',uri=True)
done=dict(json.loads(line) for line in (p/'completed.jsonl').read_text().splitlines())
groups=collections.Counter();samples=collections.defaultdict(list)
subdirs=collections.Counter();page_counts=collections.Counter()
by_area=collections.defaultdict(collections.Counter)
def signature(msg):
 for expr,kind in [(r"Can't find variable: ([\w$]+)",'missing global'),(r"([\w$.]+) is not a function",'missing callable'),(r"([\w$.]+) is not a constructor",'missing constructor')]:
  m=re.search(expr,msg)
  if m:return kind+' '+m[1]
 if msg.startswith('assert_implements:'):return 'implementation assertion'
 if 'not implemented' in msg or 'is not supported' in msg:return 'reported unsupported'
 return 'other explicit assertion'
for key in done:
 receipt=p/(key+'.json');report=json.loads(receipt.read_bytes())
 for row in report['results']:
  u=row['url'];raw,path=db.execute("select raw,path from result where label='chromeWpt' and url=?",(u,)).fetchone()
  ch=json.loads(zlib.decompress(raw));ref,_=index([(t['name'],t['status']) for t in ch['subtests']],endpoint_map(path,u)[0])
  local,_=index([(t['name'],(t['status'],t.get('msg') or '')) for t in row['subtests']],endpoint_map(str(receipt),u)[0])
  for case,(status,_) in ref.items():
   if status!='Pass':continue
   if case not in local:k='name absent on '+row['status']+' page'
   else:
    (v,msg),_=local[case]
    if v=='Pass':continue
    k=signature(msg) if v=='Fail' else 'reported '+v
   groups[k]+=1;by_area[u.split('/')[0]][k]+=1
   directory='/'.join(u.split('/')[:-1][:3])
   subdirs[directory]+=1;page_counts[u]+=1
   if len(samples[k])<3 and u not in samples[k]:samples[k].append(u)
out={'urls':sum(done.values()),'chromeOnly':sum(groups.values()),'groups':[dict(signature=k,count=v,exampleUrls=samples[k]) for k,v in groups.most_common()],'byArea':{k:v.most_common() for k,v in by_area.items()},'largestSourceSubdirectories':subdirs.most_common(60),'largestUrlGaps':page_counts.most_common(60)}
(p/'comparison/observed-gaps.json').write_text(json.dumps(out,indent=2))
print(out['urls'],out['chromeOnly'],[(k,v) for k,v in groups.most_common(8)])
