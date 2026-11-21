from pathlib import Path
import json,gzip,hashlib,collections,shutil
p=Path(__file__).resolve().parent;o=p/'evidence'
for f in ['run.sh','harness.sh','adjacent.sh','selection.json','harness.json','adjacent.json','union.json','export-accepted.py','probe.js','storage.cjs','mixed.cjs','chrome-probe.cjs','ordinary-workload.js','workload.cjs','accepted-direct.json','chrome-direct.json','accepted-storage.json','chrome-storage.json','accepted-mixed.json','chrome-mixed.json','finalMeasuredBaseA-workload.json','finalMeasuredBaseB-workload.json','finalMeasuredHeadA-workload.json','finalMeasuredHeadB-workload.json']:
 shutil.copy2(p/f,o/f)
for l in ['base','accepted','head','final']:
 for k in ['', '-harness', '-adjacent']:
  f=p/(l+k+'.json')
  if f.is_file():
   with gzip.open(o/(f.name+'.gz'),'wb') as z:z.write(f.read_bytes())
(o/'native-results.txt').write_text((p/'native-accepted.log').read_text().split('running 18 tests\n')[-1])
s=json.load(open(o/'summary.json'));counts=collections.defaultdict(collections.Counter)
pages=set();names=set()
for l in gzip.open(o/'pages.jsonl.gz','rt'):
 u,k,a,b,c=json.loads(l);assert u not in pages;pages.add(u)
 for label,r in zip(('base','accepted','chrome'),(a,b,c)):counts[k][label+'/full']+=r[2]; counts[k][label+'/passes']+=r[3]
for l in gzip.open(o/'named.jsonl.gz','rt'):
 u,n,i,a,b,c=json.loads(l);assert u in pages and (u,n,i) not in names;names.add((u,n,i))
for k,ct in counts.items():
 for field,count in ct.items():assert count==s[k][field]
assert len(pages)==90
for kind in ['direct','storage']:
 a,b=(json.load(open(p/(lab+'-'+kind+'.json'))) for lab in ['accepted','chrome'])
 if kind=='storage':a,b=a['results'],b['results']
 assert a==b
print('verified',len(pages),len(names),counts)
