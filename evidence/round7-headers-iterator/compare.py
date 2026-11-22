import json
from pathlib import Path
labels=['chrome158','observed-fada294a3','base','candidate']
docs={label:json.loads(Path(label+'.json').read_text()) for label in labels}
urls=json.loads(Path('urls.json').read_text())
rows=[]
pages=[]
for url in urls:
    names={}
    page={'url':url,'completion':{},'harnessStatus':{},'counts':{}}
    for label,doc in docs.items():
        result,=[r for r in doc['results'] if r['url']==url]
        page['completion'][label]=result['status']
        page['harnessStatus'][label]=result['harnessStatus']
        assert result['blob']=='ead1047645a15fa63fdbb665eaf8fc685b3bde7b'
        assert doc['wptRevision']=='a421f2fea5eb7f91e0db399e44b0fb0779ee1603'
        names[label]={s['name']:s['status'] for s in result['subtests']}
        assert len(names[label])==len(result['subtests'])
        page['counts'][label]={status:list(names[label].values()).count(status) for status in sorted(set(names[label].values()))}
    for name in sorted(set.union(*(set(n) for n in names.values()))):
        rows.append({'url':url,'name':name,**{k:v.get(name,'Absent') for k,v in names.items()}})
    pages.append(page)
Path('per-name.json').write_text(json.dumps(rows,indent=2)+'\n')
Path('pages.json').write_text(json.dumps(pages,indent=2)+'\n')
print(pages)
from collections import Counter
print(Counter(tuple(r[l] for l in labels) for r in rows))
