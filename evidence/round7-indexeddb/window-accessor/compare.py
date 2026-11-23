from pathlib import Path
import collections, json
d = Path(__file__).resolve().parent
def read(p):
    return json.loads(p.read_text())
old = read(d.parent/'idb-worker-1d0a43b93/outcome-comparison.json')
former = {(r['url'],r['name']) for r in old['formerChromeOnly']}
arms = ['parent','candidate','repeat','chrome']
rows = {}
pages = {}
for arm in arms:
    data = read(d/(arm+'.json'))
    for page in data['results']:
        url = page['url']
        pages.setdefault(url,{})[arm] = {k:page.get(k) for k in ['harnessStatus','error','source','blob','environment','protocol']}
        counts = collections.Counter()
        for t in page['subtests']:
            n = t['name']
            counts[n] += 1
            rows.setdefault((url,n,counts[n]),{})[arm] = t['status']
assert set(pages)==set(read(d/'selection.json'))
assert all(set(v)==set(arms) for v in rows.values())
assert all(all(all(v[a][k]==v['chrome'][k] for k in ['source','blob','environment','protocol']) for a in arms) for v in pages.values())
def named(k):
    return dict(url=k[0],name=k[1],occurrence=k[2],**rows[k])
result = dict(count=len(rows),passes={a:sum(v[a]=='Pass' for v in rows.values()) for a in arms}, changes=[named(k) for k,v in rows.items() if v['parent']!=v['candidate']],repeatDifferences=[named(k) for k,v in rows.items() if v['repeat']!=v['candidate']], chromeDifferences=[named(k) for k,v in rows.items() if v['chrome']!=v['candidate']], formerWorkerGains=[named(k) for k in rows if k[:2] in former],pages=pages)
assert len(result['formerWorkerGains'])==30
(d/'comparison.json').write_text(json.dumps(result,indent=2)+'\n')
print(result['count'],result['passes'],result['changes'],len(result['repeatDifferences']),len(result['chromeDifferences']))
