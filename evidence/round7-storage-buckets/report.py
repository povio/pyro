from pathlib import Path
import collections,gzip,hashlib,json,sys
w=Path(__file__).parent;engines=['base','candidate','chrome158'];reads=lambda f:json.loads((w/f).read_bytes());write=lambda name,x:(w/name).write_text(json.dumps(x,indent=2,ensure_ascii=False)+'\n')
runs={l:reads(l+'.json') for l in engines};pages={l:{r['url']:r for r in d['results']} for l,d in runs.items()};urls=reads('urls.json')
names=[];pp=[];area=collections.Counter();scores=collections.Counter()
def nm(r):
 seen=collections.Counter();out={}
 for x in r['subtests']:
  n=x['name'];seen[n]+=1;out[n,seen[n]]=x['status']
 return out
for u in urls:
 rs=[pages[l][u] for l in engines];pr=[];maps=[nm(r) for r in rs]
 for l,r in zip(engines,rs):
  full=r['status']=='complete' and r['harnessStatus']=='OK' and bool(r['subtests']) and all(t['status']=='Pass' for t in r['subtests'])
  pr.append([r['status'],r['harnessStatus'],full]);scores[l+'Full']+=full
 pp.append([u,*pr])
 for k in sorted(set.union(*(set(m) for m in maps))):
  st=[m.get(k) for m in maps];names.append([u,*k,*st])
  for l,v in zip(engines,st):scores[l+'Named']+=v=='Pass'
  if st[0]!='Pass' and st[1]=='Pass':area[u]+=1;scores['gained']+=1
  scores['lost']+=st[0]=='Pass' and st[1]!='Pass';scores['new']+=st[0] is None and st[1] is not None;scores['missing']+=st[0] is not None and st[1] is None
  scores['chromeOnly']+=st[1]!='Pass' and st[2]=='Pass';scores['lpOnly']+=st[1]=='Pass' and st[2]!='Pass'
with gzip.open(w/'named.jsonl.gz','wt') as f:
 for row in names:f.write(json.dumps(row,ensure_ascii=False,separators=(',',':'))+'\n')
write('pages.json',pp);summary={'scores':scores,'gainsByPage':area,'engines':{l:{'pageOutcomes':collections.Counter(r['status'] for r in runs[l]['results'] if r['url'] in urls),'restarts':runs[l]['restarts'],'watchdogKills':runs[l]['watchdogKills'],'receiptSHA256':hashlib.sha256((w/(l+'.json')).read_bytes()).hexdigest()} for l in engines}};write('summary.json',summary);print(summary)
