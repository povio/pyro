"""Attach current coverage to all official URL variants and retain omissions."""
import collections, gzip, json
from pathlib import Path
here=Path(__file__).resolve().parent
root=here.parent
read=lambda p:json.loads(p.read_bytes())
meta=read(root/'final-official-inventory.json')
by_url={t['url']:t for t in meta['eligible']+meta['inventoryOnly']}
unavailable={r['url']:r.get('reason','inventory-only') for r in meta['inventoryOnly']}
unavailable.update({r['url']:'previously reviewed outside ordinary execution' for r in read(root/'deferred-scope.json')})
unavailable.update(dict.fromkeys(read(root/'combined2-withheld-runtime.json')['urls'],'Wasm runtime withheld'))
unavailable.update({u:'historical '+s for u,s in read(here/'coverage.json')['withheld'].items()})
wire=read(root/'transport-inventory-only.json')['urls']
unavailable.update(dict.fromkeys(wire,'raw HTTP/1 transport unsupported'))
assert len(by_url)==31827
assert len(unavailable)==1314,(len(unavailable),len(wire))
results={}
with gzip.open(here/'comparison/pages.jsonl.gz','rt') as rows:
 for line in rows:
  u,env,*vals=json.loads(line);results[u]=vals
assert not unavailable.keys() & results.keys()
with gzip.open(root.parent.parent/'round6-inventory/pinned-inventory.jsonl.gz','rt') as records:
 matches={r['url'].lstrip('/'):r['stable_source_unchanged'] for line in records if (r:=json.loads(line))}
counts=collections.defaultdict(collections.Counter)
with gzip.open(here/'comparison/inventory.jsonl.gz','wt') as out:
 for u in sorted(by_url):
  r=by_url[u];reason=unavailable.get(u)
  if not reason and u not in results:reason='awaiting measurement'
  counts[r['environment']][reason or 'reported']+=1
  print(json.dumps([u,r['source'],r['blob'],r['environment'],reason,results.get(u),matches.get(u)],separators=(',',':')),file=out)
(here/'comparison/environments.json').write_text(json.dumps(counts,indent=2))
