"""Bind actual per-attempt Studio port config bytes to the completed run's server log."""
import argparse,collections,hashlib,json,re
from pathlib import Path
p=argparse.ArgumentParser();p.add_argument('source',type=Path);p.add_argument('plan',type=Path);p.add_argument('out',type=Path);p.add_argument('--manifest',type=Path);a=p.parse_args();a.out.mkdir(parents=True,exist_ok=True)
bykey=collections.defaultdict(list)
if a.manifest:
 for row in json.loads(a.manifest.read_bytes()):bykey[row['key']].append(row)
for t in json.loads(a.plan.read_text()):
 if t['owner']!='studio':continue
 key=t['key'];sub=a.source/key;log=sub/'run.log';cfg=sub/'config.json';l=log.read_bytes();logsha=hashlib.sha256(l).hexdigest()
 announced=re.findall(rb'^wptserve on http://web-platform\.test:(\d+) \(log (.*?/serve\.log)\)$',l,re.M)
 records=bykey.get(key,[]);selected=None
 if a.manifest and len(announced)==1:
  matching=[r for r in records if r.get('originalServerLog')==announced[0][1].decode() and r.get('config.json')]
  if len(matching)==1:
   r=matching[0];assert r['index']==t['index'] and Path(r['originalConfig'])==Path(r['originalServerLog']).with_name('config.json')
   assert Path(r['sourceRunLog']).parts[-2:]==(key,'run.log')
   assert Path(r['config.json']['path']).parts[-3:]==(key,'attempt-'+str(r['attempt']),'config.json')
   cfg=a.manifest.parent/key/('attempt-'+str(r['attempt']))/'config.json'
   assert hashlib.sha256(cfg.read_bytes()).hexdigest()==r['config.json']['sha256']
   selected=r
 if (a.manifest and selected is None) or not cfg.exists() or len(announced)!=1:
  proof={'logSHA256':logsha,'proof':None,'missing':'No unambiguous retained config for the completed Studio server announcement','shard':key,'attemptLog':str(log.relative_to(a.source)),'archivedAttempts':records}
 else:
  raw=cfg.read_bytes();c=json.loads(raw)
  assert c['server_host']=='127.0.0.1' and c['browser_host']=='web-platform.test' and announced[0][0]==str(c['ports']['http'][0]).encode()
  proof={'logSHA256':logsha,'proof':{'sha256':hashlib.sha256(raw).hexdigest(),'ports':c['ports']},'sourceAttempt':selected}
 (a.out/(key+'.json')).write_text(json.dumps(proof,indent=1)+'\n')
