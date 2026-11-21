"""Keep authenticated role maps before wptserve's temporary server configuration expires."""
import hashlib,json,pathlib,sys
p=pathlib.Path(__file__).resolve().parent
sys.path.insert(0,str(p.parents[1]/'round6-noncss-measure/manifest-runs/census-902fe811f'))
from wpt_ports import config,DEFAULTS
def snapshot_config(log,cache):
 log=pathlib.Path(log);cache=pathlib.Path(cache)
 h=hashlib.sha256(log.read_bytes()).hexdigest()
 if cache.exists():
  saved=json.loads(cache.read_bytes());assert saved['logSHA256']==h
  proof=saved['proof'];m={}
  for role,values in proof['ports'].items():
   if role not in DEFAULTS:continue
   proto='https' if role=='h2' else role.split('-')[0]
   for actual,standard in zip(values,DEFAULTS[role]):
    assert (proto,actual) not in m;m[proto,actual]=standard
  return m,proof
 m,pr=config(str(log));assert 'error' not in pr,pr
 saved={'logSHA256':h,'proof':{'sha256':pr['sha256'],'ports':pr['ports']}}
 cache.parent.mkdir(parents=True,exist_ok=True)
 import os,tempfile
 with tempfile.NamedTemporaryFile('w',dir=cache.parent,delete=False) as f:
  json.dump(saved,f,indent=1);f.write('\n');tmp=f.name
 os.replace(tmp,cache)
 return m,saved['proof']
if __name__=='__main__':
 for label,root in [('chrome',p/'raw'),('cbdeb',p.parents[1]/'round6-noncss-measure/manifest-runs/census-cbdeb1193/raw')]:
  if not (root/'completed.jsonl').exists():continue
  receipts=[json.loads(l) for l in (root/'completed.jsonl').read_text().splitlines() if l.strip()]
  for row in receipts:
   k=row['key'];cache=p/'verified-ports'/label/(k+'.json')
   if not cache.exists():snapshot_config(root/(k+'.log'),cache)
