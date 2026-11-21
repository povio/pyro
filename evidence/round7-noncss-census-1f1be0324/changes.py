"""Summarize all named gains and losses vs 8e; retain exact occurrence and Chrome provenance."""
from pathlib import Path
import collections,gzip,json,sys
p=Path(sys.argv[1]);counts=collections.defaultdict(collections.Counter);areas=collections.defaultdict(collections.Counter)
def rowiter(name):
 with gzip.open(p/name,'rt') as r:yield from map(json.loads,r)
def passing(x):return x is not None and x[0]=='Pass'
def state(x):return 'missing-name' if x is None else x[0]
pages={u:[h,c,b] for u,h,c,b,_,_ in rowiter('pages.jsonl.gz') if h}
with gzip.open(p/'prior-pass-changes.jsonl.gz','wt') as out:
 for u,n,i,h,c,b in rowiter('named.jsonl.gz'):
  if u not in pages or passing(h)==passing(b):continue
  sign='gain' if passing(h) else 'loss';counts[sign][state(c)]+=1;areas[u.split('/')[0]][sign]+=1
  print(json.dumps([u,n,i,sign,h,c,b,[pg[:3] for pg in pages[u]]],ensure_ascii=True,separators=(',',':')),file=out)
s=json.loads((p/'summary.json').read_bytes())['comparisons'].get('headPrevious',{})
assert sum(counts['gain'].values())==s.get('firstOnly',0) and sum(counts['loss'].values())==s.get('secondOnly',0)
(p/'prior-pass-changes.json').write_text(json.dumps(dict(chromeDisposition=counts,byArea=areas),indent=1)+'\n')
