import pathlib,json,hashlib
d=pathlib.Path(__file__).resolve().parent
c=d.parents[1]/'round6-reference-158/census'
sel=set(json.loads((d/'selection.json').read_text()))
ledger={r['key']:r for l in (c/'raw/completed.jsonl').read_text().splitlines() if (r:=json.loads(l))}
out=[]; sources={}
for s in (c/'selections').glob('ordinary-*.json'):
    hits=sel.intersection(json.loads(s.read_text()))
    if not hits: continue
    f=c/'raw'/s.name; raw=f.read_bytes(); sha=hashlib.sha256(raw).hexdigest()
    assert sha==ledger[s.stem]['receiptSHA256']
    data=json.loads(raw)
    assert data['wptRevision'].startswith('a421f2fea5eb') and data['http2'] and data['testdriver'] and data['manifestInventory']
    rows=[row for row in data['results'] if row['url'] in hits]
    assert len(rows)==len(hits)
    sources[s.name]={'path':str(f),'sha256':sha,'urls':sorted(hits)}
    out+=rows
assert len(out)==len(sel)
(d/'chrome.json').write_text(json.dumps(dict(identity=json.loads((c/'reference-identity.json').read_text()),sources=sources,results=out),indent=2))
