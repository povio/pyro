"""Reconcile historical pass losses against finished shards; rerun after the census."""
import collections
import hashlib
import json
import sqlite3
import sys
import zlib
from pathlib import Path

out = Path(__file__).resolve().parent
run = out.parent
manifest = run.parent
prior = manifest.parent / 'loss-audit-batch2'
sys.path.insert(0, str(prior))
from wpt_ports import endpoint_map, index

def load(path):
    return json.loads(path.read_bytes())

def save(file, data):
    dest = out / file
    tmp = dest.with_suffix('.tmp')
    tmp.write_text(json.dumps(data, indent=1, sort_keys=True) + '\n')
    tmp.replace(dest)

def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()

def key(row):
    return (row['url'], row['name'], row['occurrence'])

raw = {key(r): r for r in load(prior / 'losses.json')}
historical = load(prior / 'canonical-losses.json')
assert len(historical) == 292
wanted = {r['url'] for r in historical}
new = {}
proofs = {}
def unpack(receipt, row):
    mapping, proof = endpoint_map(receipt, row['url'])
    assert 'error' not in proof, proof
    proofs.setdefault(proof['sha256'], proof['ports'])
    table, collisions = index([(s['name'], s['status']) for s in row['subtests']], mapping)
    return row['status'], row.get('harnessStatus'), table, collisions, proof['sha256']

done = [json.loads(line) for line in (run / 'completed.jsonl').read_text().splitlines()]
for shard, count in done:
    selection = load(run / 'selections' / (shard + '.json'))
    if not wanted.intersection(selection):
        continue
    receipt = run / (shard + '.json')
    report = load(receipt)
    assert report['wptRevision']=='a421f2fea5eb7f91e0db399e44b0fb0779ee1603'
    assert sorted(selection)==sorted(r['url'] for r in report['results'])
    for r in report['results']:
        if r['url'] in wanted:
            assert r['url'] not in new
            new[r['url']] = unpack(receipt, r)

db = sqlite3.connect(f'file:{manifest}/scores.sqlite?mode=ro', uri=True)
chrome = {}
for url in wanted:
    path, data = db.execute("select path,raw from result where label='chromeWpt' and url=?", (url,)).fetchone()
    chrome[url] = unpack(path, json.loads(zlib.decompress(data)))
rows = []
counts = collections.Counter()
by_cause = collections.defaultdict(collections.Counter)
for item in historical:
    url = item['url']
    k = item['canonical'], item['occurrence']
    old = raw[url,item['base_name'],item['occurrence']]
    result = dict(item, classification=old['classification'], detail=old['detail'], previousPage=old['combined2_page'])
    for label, data in [('current',new.get(url)), ('chrome',chrome[url])]:
        page, harness, table, collisions, proof = data or (None,None,{}, {},None)
        status, name = table.get(k,(None,None))
        assert k[0] not in collisions
        result[label] = dict(page=page,harness=harness,status=status,name=name,configSha256=proof)
    status = result['current']['status']
    outcome = ('unmeasured-page' if url not in new else 'missing-name' if status is None else status)
    result['outcome'] = outcome
    counts[outcome] += 1
    by_cause[old['classification'] + ' / ' + old['detail']][outcome] += 1
    rows.append(result)
save('rows.json', rows)
save('server-ports.json', proofs)
summary = dict(lp='902fe811fbef4ff0f3e39ca1f896de3a275d20b5',chrome='157.0.8103.80 macOS arm64 local WPT profile',historicalCommits=['e4ca26f48','4c13e694a'], historicalLossesSha256=digest(prior/'canonical-losses.json'), losses=292, affectedPages=len(wanted), observedAffectedPages=len(new), counts=counts, classification=by_cause, finishedShards=len(done), finishedPages=sum(count for _,count in done))
save('summary.json', summary)
print(json.dumps({k:v for k,v in summary.items() if k!='classification'}))
