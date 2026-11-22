import json,pathlib,collections
d=pathlib.Path(__file__).resolve().parent
arms=['base','candidate','repeat','chrome']
rows={};sets={};cost={}
for arm in arms:
    raw=json.loads((d/(arm+'.json')).read_text())
    rows[arm]={r['url']:r for r in raw['results']}
    assert set(rows[arm])==set(json.loads((d/'selection.json').read_text()))
    sets[arm]=sorted([url,t['name']] for url,r in rows[arm].items() for t in r.get('subtests',[]) if t['status']=='Pass')
    cost[arm]={k:raw.get(k) for k in ['wallMs','browserCpuS','runnerCpuS','serverCpuS','restarts','watchdogKills'] if k in raw}
urls={}
for url in sorted(rows['chrome']):
    urls[url]={}
    for arm in arms:
        r=rows[arm][url];ref=rows['chrome'][url]
        assert all(r[k]==ref[k] for k in ('environment','protocol','blob','source'))
        urls[url][arm]={'status':r['status'],'error':r.get('error'),'pass':sum(x['status']=='Pass' for x in r.get('subtests',[])),'other':[x for x in r.get('subtests',[]) if x['status']!='Pass']}
def diff(a,b):return [k for k in sets[a] if k not in sets[b]]
summary={'counts':{a:len(sets[a]) for a in arms},'gain':diff('candidate','base'),'loss':diff('base','candidate'),'repeatGain':diff('repeat','candidate'),'repeatLoss':diff('candidate','repeat'),'cost':cost,'urls':urls,'passSets':sets}
(d/'comparison.json').write_text(json.dumps(summary,indent=2))
print({k:v for k,v in summary.items() if k not in ('urls','passSets')})
