import collections
import json
from pathlib import Path
p=Path(__file__).resolve().parent
def index(name):
    record=json.loads((p/'results'/(name+'.json')).read_text())
    names={}; pages={}; fully=[]
    for row in record['results']:
        u=row['url'];pages[u]=[row['status'],row.get('harnessStatus')];seen=collections.Counter()
        for test in row['subtests']:
            n=test['name']; seen[n]+=1;names[u,n,seen[n]]=test['status']
        if row['status']=='complete' and row.get('harnessStatus')=='OK' and row['subtests'] and all(t['status']=='Pass' for t in row['subtests']):
            fully.append(u)
    return {'pages':pages,'fullyPassingPages':sorted(fully),'statusCounts':dict(collections.Counter(names.values())),
            'namedPasses':sorted(k for k,v in names.items() if v=='Pass'),'allNames':sorted(names)}
obs={label:index(raw) for label,raw in [('base','lp-exact'),('feature','feature'),('chrome','stable'),
                                       ('supplementalBase','probes-base-all'),('supplementalFeature','probes-feature')]}
def delta(a,b):
    src=obs[a];dst=obs[b]
    assert src['pages'].keys()==dst['pages'].keys()
    def keys(x,k):return {tuple(v) for v in x[k]}
    return {'gainedNamed':sorted(keys(dst,'namedPasses')-keys(src,'namedPasses')),
            'lostNamed':sorted(keys(src,'namedPasses')-keys(dst,'namedPasses')),
            'missingNamed':sorted(keys(src,'allNames')-keys(dst,'allNames')),
            'addedNamed':sorted(keys(dst,'allNames')-keys(src,'allNames')),
            'changedStatuses':{k:[src['pages'][k],v] for k,v in dst['pages'].items() if v!=src['pages'][k]}}
out={'observations':obs,'officialDelta':delta('base','feature'),'supplementalDelta':delta('supplementalBase','supplementalFeature')}
(p/'results'/'comparison.json').write_text(json.dumps(out,indent=1)+'\n')
