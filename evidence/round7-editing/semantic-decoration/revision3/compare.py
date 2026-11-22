import json, collections
from pathlib import Path
root=Path(__file__).resolve().parents[1]
for suite,folder,r1,r2 in [('semantic',root,'head','corrected'),('ordinary',root/'ordinary','first','head')]:
    arms={key:json.loads((folder/f'scripts-{name}.json').read_text())['rows'] for key,name in [('signed','chrome'),('parent','base'),('first',r1),('second',r2),('candidate','final')]}
    results={}
    losses=[]
    atomic=[]
    for index,case in enumerate(arms['candidate']):
        fixture={key:case[key] for key in ['target','shape','css','reverse','selection']}
        for stage in ['after','twice','onceUndo','restored']:
            for field,actual in case[stage].items():
                signed=arms['signed'][index][stage][field]
                totals=results.setdefault(stage,collections.Counter())
                totals[field]+=actual==signed
                previous=[arm for arm in ['parent','first','second'] if arms[arm][index][stage][field]==signed]
                if actual!=signed and previous:
                    losses.append(dict(fixture=fixture,stage=stage,field=field,previous=previous,actual=actual,signed=signed))
        atomic.append(dict(fixture=fixture,fields={field:case['onceUndo'][field]==value for field,value in case['after'].items()}))
    dest=root/'revision3'
    for suffix,data in [('totals',results),('losses',losses),('atomic',atomic)]:
        (dest/f'{suite}-{suffix}.json').write_text(json.dumps(data,indent=2)+'\n')
    print(suite,results)
    print('losses',collections.Counter((r['stage'],r['field']) for r in losses if r['stage']!='onceUndo'))
