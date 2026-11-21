"""Pair the late 152-page 6c6045cfe feature run with the corresponding integrated head rows."""
from pathlib import Path
import collections,gzip,hashlib,json,sys
p=Path(__file__).resolve().parent;out=Path(sys.argv[1]);m=p.parents[1]
sys.path.insert(0,str(m/'manifest-runs/census-902fe811f'))
from wpt_ports import config,index
oldfile=m/'reconciled/round6-284-focus-old.json';r=json.loads(oldfile.read_bytes())
assert r['jobs']==4 and r['http2'] and r['testdriver'] and r['wptRevision']=='a421f2fea5eb7f91e0db399e44b0fb0779ee1603'
assert r['inventoryPlan']['viewport']=={'width':800,'height':600,'deviceScaleFactor':1}
sha=lambda f:hashlib.sha256(f.read_bytes()).hexdigest()
assert sha(Path(r['bin']))==(p.parent/'wave2/final.sha256').read_text().split()[0]
assert (p.parent/'wave2/final-head.txt').read_text().strip().startswith('6c6045cfe')
mp,configproof=config(str(p/'focus-old.log'));assert 'error' not in configproof
planned=json.loads((p/'noncss-focus.json').read_bytes());old={x['url']:x for x in r['results']};assert len(old)==len(planned)==152 and set(old)==set(planned)
def rows(file):
 for l in gzip.open(out/file,'rt'):yield json.loads(l)
def put(f,r):f.write(json.dumps(r,separators=(',',':'))+'\n')
new={};counts=collections.Counter()
with gzip.open(out/'prior-focus-pages.jsonl.gz','wt') as pg,gzip.open(out/'prior-focus-names.jsonl.gz','wt') as nf:
 for u,k,co,scope,d,b,h,c in rows('pages.jsonl.gz'):
  if u not in old:continue
  t=old[u];assert d==[t[a] for a in ('source','blob','environment','protocol')]
  tbl,coll=index([(x['name'],x['status']) for x in t['subtests']],mp);assert not coll
  s=sum(v[0]=='Pass' for v in tbl.values());op=[t['status'],t['harnessStatus'],bool(tbl) and t['status']=='complete' and t['harnessStatus']=='OK' and len(tbl)==s,s,len(tbl)]
  put(pg,[u,op,h]);new[u]=[tbl,h]
  if h:
   counts['paired']+=1;counts['priorFull']+=op[2];counts['headFull']+=h[2];counts['priorPass']+=s;counts['headPass']+=h[3]
   counts['fullGains']+=h[2] and not op[2];counts['fullLosses']+=op[2] and not h[2]
 def named(u,n,i,a,b):
  if a is None and b is None:return
  put(nf,[u,n,i,a,b]);ap=a is not None and a[0]=='Pass';bp=b is not None and b[0]=='Pass'
  if new[u][1]:
   counts['namedGains']+=bp and not ap;counts['namedLosses']+=ap and not bp;counts['common']+=ap and bp
   counts['newNames']+=a is None;counts['missingNames']+=b is None
 for u,n,i,b,h,c in rows('named.jsonl.gz'):
  if u in old:named(u,n,i,new[u][0].pop((n,i),None),h)
 for u,(tbl,_) in new.items():
  for (n,i),a in tbl.items():named(u,n,i,a,None)
assert len(new)==len(old)
result=dict(counts=counts,prior='6c6045cfe4d1fb637ad7ab49dbab2fff02958026',replay=(p/'commit.txt').read_text().strip(),priorBinarySHA256=sha(Path(r['bin'])),priorReceiptSHA256=sha(oldfile),priorLogSHA256=sha(p/'focus-old.log'),priorConfigSHA256=configproof['sha256'],priorPorts=configproof['ports'],selectionSHA256=sha(p/'noncss-focus.json'))
(out/'prior-focus-summary.json').write_text(json.dumps(result,indent=1)+'\n')
