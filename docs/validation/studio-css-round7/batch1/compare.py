import argparse
import gzip
import hashlib
import json
from collections import Counter
from pathlib import Path


def read(path):
    data = path.read_bytes()
    return json.loads(gzip.decompress(data) if path.suffix == '.gz' else data)


def save(path, data):
    encoded = json.dumps(data, sort_keys=True, separators=(',', ':')).encode()
    path.write_bytes(gzip.compress(encoded, mtime=0) if path.suffix == '.gz' else encoded)


def sets(outcomes):
    pages, passed, observed = set(), set(), set()
    states = {}
    for url, (status, harness, assertions) in outcomes.items():
        if status == 'complete' and harness == 0 and assertions and all(result == 'Pass' for _, result in assertions):
            pages.add(url)
        for name, result in assertions:
            observed.add((url, name))
            if result == 'Pass':
                passed.add((url, name))
        states.setdefault(f'{status}:{harness}', set()).add(url)
    return {'pages': pages, 'passes': passed, 'observed': observed, **states}


def compare(raw_dir, target, cohort):
    parent = Path(__file__).resolve().parents[1] / 'frozen'
    urls = read(parent / f'{cohort}.json.gz')
    assert len(urls) == len(set(urls)) and not set(urls).intersection(read(parent / 'withheld.json.gz'))
    raw_path = raw_dir / f'{cohort}.json'
    raw = read(raw_path)
    assert raw['jobs'] == 2 and not raw['skipped'] and not raw['reuseContext']
    assert raw['bin'] == str((raw_dir/'candidate').resolve()) and raw['discovered'] == len(urls)
    assert raw['wptRevision'] == 'a421f2fea5eb7f91e0db399e44b0fb0779ee1603'
    assert raw['dirs'] == ['css'] and raw['kind'] == 'testharness' and raw['restricted']
    assert raw['server'] == 'wptserve' and raw['testdriver'] and raw['workerGlobals'] == (cohort == 'supplement')
    assert Counter(raw['plan']) == Counter(urls) == Counter(row['url'] for row in raw['results'])
    outcomes = {row['url']: [row['status'], row.get('harnessStatus'), [[sub['name'],sub['status']] for sub in row['subtests'] if sub['name'] != 'no test suite completion']] for row in raw['results']}
    save(target / f'outcomes-{cohort}.json.gz', outcomes)
    candidate = sets(outcomes)
    summary = {'candidate': {key:len(value) for key,value in candidate.items()}, 'wallMs':raw['wallMs'], 'restarts':raw['restarts'], 'urls':len(urls), 'rawSha256':hashlib.sha256(raw_path.read_bytes()).hexdigest()}
    for label in ['frozen', 'chrome']:
        data = read(parent / f'outcomes-{label}-{cohort}.json.gz')
        assert set(data) == set(outcomes)
        reference = sets(data)
        directions = {}
        for key in candidate.keys() | reference.keys():
            left, right = candidate.get(key,set()), reference.get(key,set())
            directions[key] = {'candidateOnly':sorted(left-right),'referenceOnly':sorted(right-left)}
        save(target / f'difference-{cohort}-vs-{label}.json.gz', directions)
        summary['vs-'+label] = {key:{side:len(items) for side,items in value.items()} for key,value in directions.items()}
    save(target / f'summary-{cohort}.json', summary)


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('raw_dir', type=Path)
    parser.add_argument('target', type=Path)
    parser.add_argument('cohorts', choices=['main','supplement'], nargs='+')
    args = parser.parse_args()
    for cohort in args.cohorts:
        compare(args.raw_dir, args.target, cohort)
