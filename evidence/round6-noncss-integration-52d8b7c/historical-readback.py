"""Read historic names from raw receipts independently of the combined exporter."""
import collections,json,sys,gzip
from pathlib import Path
p=Path(__file__).resolve().parent;sys.path.insert(0,str(p.parents[1]/'manifest-runs/census-902fe811f'))
from wpt_ports import config,index
old=json.loads((p.parent/'integration-284548/verified/historical-292.json').read_bytes())
byurl=collections.defaultdict(list)
for r in old:byurl[r['url']].append(r)
data={tuple(r[k] for k in ['url','canonical','occurrence']):{} for r in old}
for label in ['base','head']:
 for f in (p/('raw-'+label)).glob('*.json'):
  rows=json.loads(f.read_bytes())['results']
  if not any(r['url'] in byurl for r in rows):continue
  ports,pr=config(str(f.with_suffix('.log')));assert 'error' not in pr
  for r in rows:
   if r['url'] not in byurl:continue
   nms,collisions=index([(s['name'],s['status']) for s in r['subtests']],ports);assert not collisions
   for h in byurl[r['url']]:
    k=(h['url'],h['canonical'],h['occurrence']); assert label not in data[k]
    data[k][label]=(nms[k[1:]][0] if k[1:] in nms else 'missing-name')
count=collections.Counter();detailed=collections.defaultdict(collections.Counter)
for r in old:
 k=(r['url'],r['canonical'],r['occurrence']);fresh=data[k]
 if 'base' not in fresh or 'head' not in fresh:
  count['unmatched']+=1; continue
 for lab in ['base','head']:
  was=r[lab][0] if r[lab] else ('missing-name' if r[lab+'Page'] else 'unobserved-page')
  detailed[lab][was+' => '+fresh[lab]]+=1
 count['paired']+=1
print(json.dumps(dict(rows=count,transitions=detailed),indent=1))
