from collections import Counter
from pathlib import Path
import json
def read(p):
    return json.loads(Path(p).read_text())
def rows(x):
    out={}
    for p in x['results']:
        assert p['status']=='complete' and p['harnessStatus']==0
        n=Counter()
        for r in p['subtests']:
            k=r['name']; n[k]+=1;out[p['url'],k,n[k]]=r['status']
    return out
b,h,c=map(read,['base-38.json','head-38.json','../designmode-plaintext/chrome.json'])
assert b['wptRevision']==h['wptRevision']==c['wptRevision']
assert b['plan']==h['plan'] and len(h['plan'])==38
B,H,C=map(rows,[b,h,c]);assert B.keys()==H.keys()
print('occurrences',len(B),len(C),'missing Chrome',len(B.keys()-C.keys()),'extra Chrome',len(C.keys()-B.keys()))
for k in B:
    if B[k]!=H[k]: print(json.dumps([*k,B[k],H[k],C.get(k)]))
for prefix,ref in [('qa','reference'),('direct','chrome'),('split','chrome')]:
    base,head,chrome=[read(f'{prefix}-{s}.json')['rows'] for s in ['base','head',ref]]
    assert len(base)==len(head)==len(chrome)
    print(prefix,len(head),'exact base/head',sum(a==c for a,c in zip(base,chrome)),sum(a==c for a,c in zip(head,chrome)),'undo restored',sum(a.get('before',a.get('html'))==a['restored'] for a in head))
    for a,b,c in zip(base,head,chrome):
        if (a!=b and b!=c) or (prefix=='qa' and b!=c): print('difference',a,b,c)
