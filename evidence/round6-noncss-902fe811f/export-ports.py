"""Archive authenticated per-page port maps from the exact producing receipts."""
from pathlib import Path
import gzip,json,sqlite3,sys
here=Path(__file__).resolve().parent;root=here.parent;out=here/'comparison'
from wpt_ports import endpoint_map
db=sqlite3.connect(f'file:{root}/scores.sqlite?mode=ro',uri=True)
configs={};done=dict(json.loads(line) for line in (here/'completed.jsonl').read_text().splitlines())
shards={u:k for k in done for u in json.loads((here/'selections'/(k+'.json')).read_bytes())}
def config(path,u):
 mapping,proof=endpoint_map(str(path),u)
 assert 'error' not in proof,proof
 sha=proof['sha256']
 if sha not in configs:
  configs[sha]=dict(ports=proof['ports'],browserHost='web-platform.test',serverHost='127.0.0.1')
 return sha
with gzip.open(out/'pages.jsonl.gz','rt') as pages,gzip.open(out/'port-provenance.jsonl.gz','wt') as dst:
 for line in pages:
  u,*_=json.loads(line)
  paths=[str(here/(shards[u]+'.json'))]+[db.execute('select path from result where label=? and url=?',(label,u)).fetchone()[0] for label in ['combined2','chromeWpt']]
  print(json.dumps([u,*[config(path,u) for path in paths]],separators=(',',':')),file=dst)
with gzip.open(out/'port-configs.json.gz','wt') as dst:json.dump(configs,dst,separators=(',',':'))
print('Archived',len(configs),'authenticated port configurations.')
