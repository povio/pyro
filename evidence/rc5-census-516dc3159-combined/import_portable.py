"""Translate authenticated Studio claim/receipt bundles for the original V2 exporter."""
from argparse import ArgumentParser
from collections import Counter
from pathlib import Path
import gzip,hashlib,json,shutil
p=ArgumentParser();p.add_argument('--source',type=Path,required=True);p.add_argument('--ports',type=Path,required=True);p.add_argument('--plan',type=Path,required=True);p.add_argument('--out',type=Path,required=True);a=p.parse_args()
def read(p):return json.loads(p.read_bytes())
def sha(p):
 with p.open('rb') as f:return hashlib.file_digest(f,'sha256').hexdigest()
def save(p,obj):
 p.parent.mkdir(parents=True,exist_ok=True);p.write_text(json.dumps(obj,indent=2,sort_keys=True)+'\n')
def copy(f,p):p.parent.mkdir(parents=True,exist_ok=True);shutil.copyfile(f,p)
assert sha(a.plan/'tasks.json')=='4a3badc8428aa0743c1747b1aecf70082d30e1a69cd9e6a17d8ce200eeb6a63e'
tasks=[t for t in read(a.plan/'tasks.json') if t['owner']=='studio']
assert len(tasks)==160 and [t['index'] for t in tasks]==list(range(1,320,2))
a.out.mkdir(exist_ok=False);raw=a.out/'raw';raw.mkdir();rows=[];binpaths=set();hosts=set()
for t in tasks:
 k=t['key'];src=a.source/k;c=read(src/'claim.json');end=read(src/'receipt.json');assert all(end.get(f)==v for f,v in c.items())
 assert (c['tag'],c['workflow'],c['task'],c['owner'],c['index'],c['selectorSHA256'])==('0.0.50-rc.5',44998826328,k,'studio',t['index'],t['sha256'])
 for key,digest in [('binarySHA256','9b2454c277a52452150d59b33c64c7d46f0206781c0068f5a6b0dd0631ffd87e'),('runnerSHA256','b82c52329f881a5193e21ceb22634517c4cb776bd39bb080692c24eb21ac594a'),('inventorySHA256','34a67512a84d43630f7adbecebd4378df5f410446591015cc6a20a3034e02819'),('wpt','a421f2fea5eb7f91e0db399e44b0fb0779ee1603')]:assert c[key]==digest
 assert end['traceSHA256']==sha(src/'trace.jsonl') and end['urls']==t['count']
 if (src/'result.json').is_file():
  r=read(src/'result.json');assert sha(src/'result.json')==end['resultSHA256'];copy(src/'result.json',raw/(k+'.json'))
 else:
  gz=src/'result.json.gz'
  with gzip.open(gz,'rb') as f:data=f.read()
  assert hashlib.sha256(data).hexdigest()==end['resultSHA256'];r=json.loads(data);copy(gz,raw/(k+'.json.gz'))
 binpaths.add(r['bin']);hosts.add(c['host'])
 assert set(r['plan'])==set(read(a.plan/'selections'/(k+'.json')))=={v['url'] for v in r['results']}
 proof=read(a.ports/(k+'.json'));assert proof['logSHA256']==sha(src/'run.log')
 copy(a.ports/(k+'.json'),a.out/'verified-ports'/(k+'.json'));copy(src/'run.log',raw/(k+'.log'))
 save(raw/(k+'.start.json'),dict(key=k,ordinal=c['index'],selectorSHA256=c['selectorSHA256'],artifactSHA256=c['binarySHA256'],sourceClaimSHA256=sha(src/'claim.json')))
 for n in ['claim.json','receipt.json','trace.jsonl']:copy(src/n,a.out/'original'/k/n)
 rows.append(dict(key=k,owner='studio',index=t['index'],count=t['count'],selectionSHA256=t['sha256'],receiptSHA256=end['resultSHA256'],logSHA256=proof['logSHA256'],configSHA256=(proof['proof'] or {}).get('sha256'),statuses=Counter(v['status'] for v in r['results']),wallMs=r['wallMs'],restarts=r['restarts'],watchdogKills=r['watchdogKills']))
assert len(binpaths)==len(hosts)==1
(raw/'completed.jsonl').write_text(''.join(json.dumps(row,separators=(',',':'))+'\n' for row in rows))
save(a.out/'machine-provenance.json',dict(head='516dc3159ff4602bd3a11e98607c262c0ffc65c4',tree='39850661325fc24f3cd9e9f591d14df55a5fa359',machine='studio',host=hosts.pop(),bin=binpaths.pop(),binarySHA256=c['binarySHA256'],runnerSHA256=c['runnerSHA256'],manifestSHA256=c['inventorySHA256'],effectiveManifestSHA256=sha(a.plan/'tasks.json'),wpt=c['wpt'],sourceTag=c['tag'],workflowRunID=c['workflow'],artifact=c['asset'],derivedFrom='original per-shard claim and receipt; original files and checksums preserved'))
