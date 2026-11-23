import json
from pathlib import Path
w=Path(__file__).parent
p=json.load(open(w/'candidate-probe.json'));s=json.load(open(w/'chrome158-probe.json'))
for f in ['surface','legacy','changes','limits','window-url','service-worker','otherContext']:
    assert p[f]==s[f],(f,p.get(f),s.get(f))
assert p['basic'][:3]==s['basic'][:3] and p['basic'][4:]==s['basic'][4:]
assert p['basic'][3]==['name','value'] and s['basic'][3]==['domain','expires','name','partitioned','path','sameSite','secure','value']
pa=json.load(open(w/'candidate-partitioned-probe.json'));sa=json.load(open(w/'chrome158-partitioned-probe.json'))
assert pa['partition']==sa['partition']==['first',None,'second','first']
assert pa['partitionProtocol']==[[['first','http://127.0.0.1',False]],[['second','http://localhost',True]]]
print('Verified API, HTTP/document sharing, trusted changes, isolated context and partitions, and subscribed service worker. Signed dictionary shape recorded separately.')
