"""Quarantine port-bearing occurrence comparisons lacking actual role/port evidence."""
from pathlib import Path
from collections import Counter
import json,gzip,sys
sys.path.insert(0,str(Path(__file__).resolve().parent/'exporter'))
from wpt_ports import ENC
def audit(src,out):
 provenance=json.loads((src/'current-provenance.json').read_bytes())
 unknown={k:v['unresolvedPortMapping'] for k,v in provenance.items() if 'unresolvedPortMapping' in v}
 with gzip.open(src/'pages.jsonl.gz','rt') as f:pages={r[0]:r[4][0] for r in map(json.loads,f) if r[4][0] in unknown}
 def rows(path):
  with gzip.open(path,'rt') as f:yield from map(json.loads,f)
 def ambiguous(row):
  return row[0] in pages and any(cell is not None and ENC.search(cell[1]) for cell in row[3:6])
 counts=Counter();bykey=Counter()
 with (out/'unresolved-mappings.jsonl.gz').open('wb') as raw,gzip.GzipFile(filename='',fileobj=raw,mode='wb',mtime=0) as f:
  if unknown:
   for r in rows(src/'named.jsonl.gz'):
    if not ambiguous(r):continue
    bykey[pages[r[0]]]+=1
    for engine,c in zip(('LP','Chrome','historicalLP'),r[3:6]):
     if c is not None:
      counts[engine+'Observed']+=1;counts[engine+'Pass']+=c[0]=='Pass'
    f.write(json.dumps([pages[r[0]],r],separators=(',',':')).encode()+b'\n')
 for label in ('lp','chrome'):
  source=out/(label+'-only-names.jsonl.gz');confirmed=out/(label+'-only-confirmed-names.jsonl.gz')
  with confirmed.open('wb') as raw,gzip.GzipFile(filename='',fileobj=raw,mode='wb',mtime=0) as f:
   for r in rows(source):
    if ambiguous(r):counts[label+'UnresolvedAsymmetric']+=1;continue
    f.write(json.dumps(r,separators=(',',':')).encode()+b'\n');counts[label+'ConfirmedOnly']+=1
 result=dict(unresolvedConfigs=unknown,unresolvedRowsByShard=bykey,counts=counts)
 (out/'port-mapping-audit.json').write_text(json.dumps(result,sort_keys=True,indent=2)+'\n')
 return result
if __name__=='__main__':audit(Path(sys.argv[1]),Path(sys.argv[2]))
