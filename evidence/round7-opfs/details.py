from pathlib import Path
import json,collections,gzip,hashlib,subprocess
w=Path(__file__).parent
j=lambda f:json.loads((w/f).read_text())
if (w/'candidate-probe.json').exists():
    base,cand,ref=(j(l+'-probe.json') for l in ['base','candidate','chrome158'])
    def diff(a,b,prefix=''):
        if isinstance(a,dict) and isinstance(b,dict):
            return [d for k in sorted(set(a)|set(b)) for d in diff(a.get(k),b.get(k),prefix+'.'+k)]
        return [[prefix.removeprefix('.'),a,b]] if a!=b else []
    (w/'direct-differences.json').write_text(json.dumps(diff(cand,ref),indent=2)+'\n')
if (w/'named.jsonl.gz').exists():
    rows=list(map(json.loads,gzip.open(w/'named.jsonl.gz')))
    out={}
    for label in ['opfs','storage']:
        selected=[r for r in rows if r[0].startswith('fs/')==(label=='opfs')]
        counts=collections.Counter((b,h,c) for u,n,i,b,h,c in selected if b!=h)
        out[label]=[[*k,v] for k,v in counts.items()]
    (w/'named-transitions.json').write_text(json.dumps(out,indent=2)+'\n')
