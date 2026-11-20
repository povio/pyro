"""Expand the earlier empirical named universe with the frozen LP census, retaining its Wasm assumption."""
import gzip,json,collections
from pathlib import Path
p=Path(__file__).resolve().parent;out=p/'comparison';prior=p.parent/'final'
def records(path):
 with gzip.open(path,'rt') as f:
  for line in f:yield json.loads(line)
potential=collections.defaultdict(set)
for u,n,i,head,*refs in records(out/'named.jsonl.gz'):
 if head is not None and not any(refs):
  potential[u].add((n,i))
replacement=set()
for u,n,i,*_ in records(prior/'audited-replacement-pages.jsonl.gz'):
 replacement.add(u)
 if u in potential:potential[u].discard((n,i))
for u,n,i,*_ in records(prior/'names.jsonl.gz'):
 if u in potential and u not in replacement:potential[u].discard((n,i))
s=json.loads((prior/'summary.json').read_bytes());added=sum(map(len,potential.values()));old=s['observedNamedUniverse']['audited'];total=old+added
bounds={}
for ref,v in s['conditionalWasmBounds'].items():
 unavailable=v['unchangedWasmReferenceOnly'];passes=s['totals'][ref]['passes']
 bounds[ref]=dict(referencePasses=passes,previousWasmReferenceOnly=unavailable,maximumGivenObservedUniverse=total-unavailable,shortfall=passes-(total-unavailable))
x=dict(originalObservedUniverse=old,additionalNamesAtFrozenHead=added,expandedObservedUniverse=total,conditionalBounds=bounds,scope='Non-CSS only. Counts grant LP every observed named occurrence except the prior Wasm reference-only set. Runtime-generated names beyond these runs remain unknown. Later feature topics were not censused in full.')
(out/'conditional-bounds.json').write_text(json.dumps(x,indent=2))
print(json.dumps(x))
