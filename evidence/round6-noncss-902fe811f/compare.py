"""Compare completed receipts with prior LP and Chrome on identical URLs."""
import collections, gzip, json, sqlite3, sys, zlib
from pathlib import Path
here = Path(__file__).resolve().parent
root = here.parent
sys.path.insert(0, str(root.parent / 'loss-audit-batch2'))
from wpt_ports import endpoint_map, index
db = sqlite3.connect(f'file:{root}/scores.sqlite?mode=ro', uri=True)
counts = collections.Counter()
ref_counts = collections.defaultdict(collections.Counter)
tasks = json.loads((root/'tasks.json').read_bytes())
seen = set()
areas = collections.defaultdict(collections.Counter)
finished = dict(json.loads(line) for line in (here/'completed.jsonl').read_text().splitlines())
out = here/'comparison'
out.mkdir(exist_ok=True)
def summarize(row, path):
    mapping, audit = endpoint_map(path, row['url'])
    assert 'error' not in audit, audit
    names, collisions = index(((s['name'], s['status']) for s in row['subtests']), mapping)
    assert not collisions, (row['url'], collisions)
    passed = {k for k,(status,_) in names.items() if status=='Pass'}
    full = bool(names) and row['status']=='complete' and row['harnessStatus']=='OK' and len(passed)==len(names)
    return names, passed, full
with gzip.open(out/'named.jsonl.gz','wt') as names_out, gzip.open(out/'pages.jsonl.gz','wt') as pages_out:
 for key in tasks:
    receipt = here/(key+'.json')
    if key not in finished:continue
    doc = json.loads(receipt.read_bytes())
    assert doc['wptRevision']=='a421f2fea5eb7f91e0db399e44b0fb0779ee1603'
    assert set(doc['plan'])=={r['url'] for r in doc['results']}
    for row in doc['results']:
        u=row['url']
        assert u not in seen
        seen.add(u)
        head,hpass,hfull = summarize(row,str(receipt))
        counts.update(pages=1, namedPass=len(hpass), fullyPassing=int(hfull))
        counts['page_'+row['status']]+=1
        cols=[head]; page=[[row['status'],row['harnessStatus'],hfull]]
        for label in ('combined2','chromeWpt'):
            raw,path=db.execute('select raw,path from result where label=? and url=?',(label,u)).fetchone()
            ref=json.loads(zlib.decompress(raw))
            assert row['blob']==ref['blob']
            rs,rpass,rfull=summarize(ref,path)
            if label=='chromeWpt':areas[u.split('/')[0]].update(pages=1,lpOnly=len(hpass-rpass),chromeOnly=len(rpass-hpass),lpFull=int(hfull),chromeFull=int(rfull))
            cols.append(rs);page.append([ref['status'],ref['harnessStatus'],rfull])
            ref_counts[label].update(onlyHead=len(hpass-rpass),onlyRef=len(rpass-hpass),both=len(hpass&rpass),onlyHeadFull=int(hfull and not rfull),onlyRefFull=int(rfull and not hfull),refFull=int(rfull))
        print(json.dumps([u,row['environment'],*page],separators=(',',':')),file=pages_out)
        for n,i in sorted(set().union(*cols)):
            print(json.dumps([u,n,i,*[c.get((n,i)) for c in cols]],separators=(',',':')),file=names_out)
coverage=json.loads((here/'coverage.json').read_bytes())
summary=dict(counts=counts,comparisons=ref_counts,selected=coverage['selected'],notYetReported=coverage['selected']-len(seen),columns=['902fe811f','4c13e694a','Chrome stable 157.0.8103.80'])
(out/'areas.json').write_text(json.dumps(areas,indent=2))
(out/'summary.json').write_text(json.dumps(summary,indent=2))
print(json.dumps(summary))
