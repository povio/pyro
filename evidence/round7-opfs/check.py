from pathlib import Path
import json
w=Path(__file__).parent
load=lambda f:json.loads((w/f).read_text())
cand=load('candidate-probe.json');signed=load('chrome158-probe.json')
for field in ['surface','main','queue','worker','reload','sameContext','otherOrigin','otherContext']:
    assert cand[field]==signed[field], (field,cand[field],signed[field])
assert cand['crossRealm']==dict(same=True,mode='siloed',text='oX',types=['text/plain',''],deleted='NotFoundError')
assert signed['crossRealm']==dict(same=True,mode='siloed',text='NotFoundError',types=['text/plain',''],deleted='accepted')
c,r=load('candidate-locks.json'),load('chrome158-locks.json')
for k in r.keys()-{'exposed'}:
    assert c[k]==r[k],(k,c[k],r[k])
for idx in [0,1]:
    assert c['exposed'][idx]==r['exposed'][idx],c['exposed'][idx]
print('OPFS byte, rollback, context/origin, worker sync, same-origin receiver and seven mutation lifecycle checks passed; two signed stale-file differences retained.')
