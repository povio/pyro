import json
from pathlib import Path
w=Path(__file__).parent
for label in ('base','candidate','chrome158'):
    r=json.load(open(w/(label+'-task.json')))
    for where in ('window','worker'):
        assert len(r[where]['out'])==12 and len(r[where]['validation'])==3
        for method,depth,settled,byte in r[where]['out']:
            if label=='candidate':assert (settled,byte)==(False,9),(label,where,method,depth,settled,byte)
            if label=='chrome158':assert (settled,byte)==(False,1)
            if label=='base':assert (settled,byte)==(depth>=4,9 if depth==0 else 1)
        assert all(e==('SyntaxError' if label=='chrome158' else 'TypeError') for _,e in r[where]['validation'])
print('Verified 24 write cases per engine, including settlement state and committed bytes.')
