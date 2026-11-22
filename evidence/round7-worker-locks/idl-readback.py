from pathlib import Path
import json
w=Path(__file__).resolve().parent;read=lambda p:json.loads(p.read_bytes())
paths={'base':w/'base.json','broker':w/'broker-only/candidate.json','candidate':w/'candidate.json','chrome158':w/'chrome158.json'}
wanted={'Lock interface object length','LockManager interface object length','Lock interface: attribute name','Lock interface: attribute mode'}
rows={}
for label,p in paths.items():
 for r in read(p)['results']:
  if not r['url'].startswith('web-locks/idlharness.https.any'):continue
  for t in r['subtests']:
   if t['name'] in wanted:
    val=rows.setdefault((r['url'],t['name']),{});assert label not in val;val[label]=t['status']
assert len(rows)==16 and all(v.get('broker')=='Fail' and v.get('candidate')=='Pass' and v.get('chrome158')=='Pass' for v in rows.values())
(w/'idl-readback.json').write_text(json.dumps([{'url':u,'name':n,'status':v} for (u,n),v in sorted(rows.items())],indent=2)+'\n')
