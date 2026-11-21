import gzip
import hashlib
import json
import sys
from collections import Counter
from pathlib import Path

root = Path(__file__).resolve().parent
cache = root.parents[1] / 'css-th-e4ca26'
pin = 'a421f2fea5eb7f91e0db399e44b0fb0779ee1603'

def load(path):
    data = path.read_bytes()
    return json.loads(gzip.decompress(data) if path.suffix == '.gz' else data)

def save(name, value):
    path = root / name
    data = json.dumps(value, separators=(',', ':'), sort_keys=True).encode()
    path.write_bytes(gzip.compress(data, mtime=0) if path.suffix == '.gz' else data)

def digest(path):
    return dict(bytes=path.stat().st_size, sha256=hashlib.sha256(path.read_bytes()).hexdigest())

def collect(files, urls, worker_globals):
    outcomes = {}
    for path in files:
        raw = load(path)
        assert raw['wptRevision'] == pin and raw['kind'] == 'testharness'
        assert raw['server'] == 'wptserve' and raw['testdriver'] and raw['restricted']
        assert bool(raw.get('workerGlobals')) == worker_globals and not raw['skipped']
        rows = raw['results']
        assert Counter(raw['plan']) == Counter(row['url'] for row in rows)
        for row in rows:
            url = row['url']
            assert url in urls and url not in outcomes
            outcomes[url] = [row['status'], row.get('harnessStatus'), [[sub['name'], sub['status']] for sub in row['subtests'] if sub['name'] != 'no test suite completion']]
    assert set(outcomes) == urls
    return outcomes

def stats(data):
    pages, passed, seen = set(), set(), set()
    for url, (status, harness, assertions) in data.items():
        if status == 'complete' and harness == 0 and assertions and all(result == 'Pass' for _, result in assertions):
            pages.add(url)
        for name, result in assertions:
            seen.add((url, name))
            if result == 'Pass':
                passed.add((url, name))
    return dict(pages=pages, passes=passed, observed=seen)

def run(cohort):
    supplement = cohort == 'supplement'
    urls = set(load(root / (cohort+'.json')))
    withheld = set(load(root/'withheld.json'))
    assert not urls & withheld
    chromes = [cache/'supplement-chrome-cert.json'] if supplement else [cache/f'strict-chrome-{i}.json' for i in range(16)]
    chrome = collect(chromes, urls, supplement)
    previous = collect([cache/f'cssrootcycle-{cohort}.json'], urls, supplement)
    assert chrome == load(cache/f'outcomes-cssrootcycle-{cohort}-stable.json.gz')
    assert previous == load(cache/f'outcomes-cssrootcycle-{cohort}-candidate.json.gz')
    frozenpath = root/f'frozen-{cohort}.json'
    results = dict(chrome=chrome, historical=previous)
    if frozenpath.exists():
        results['frozen'] = collect([frozenpath], urls, supplement)
    summary = {}
    sets = {label:stats(data) for label,data in results.items()}
    for label,data in results.items():
        filename = f'outcomes-{label}-{cohort}.json.gz'
        save(filename, data)
        summary[label] = {**{k:len(v) for k,v in sets[label].items()}, 'states': dict(Counter(f'{status}:{harness}' for status,harness,_ in data.values())), **digest(root/filename)}
    if 'frozen' in sets:
        for ref in ['chrome','historical']:
            diff = {k:{'frozenOnly':sorted(v-sets[ref][k]),'referenceOnly':sorted(sets[ref][k]-v)} for k,v in sets['frozen'].items()}
            save(f'difference-{cohort}-vs-{ref}.json.gz',diff)
            summary['vs-'+ref] = {k:{key:len(values) for key,values in v.items()} for k,v in diff.items()}
    save(f'summary-{cohort}.json',summary)
    print(cohort,json.dumps(summary,indent=2))

for cohort in sys.argv[1:]:
    run(cohort)
