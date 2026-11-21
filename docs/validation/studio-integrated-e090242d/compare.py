import collections
import gzip
import json
import sys
from pathlib import Path


out, previous, frozen, focused = map(Path, sys.argv[1:])
sys.path.insert(0, str(frozen))
del sys.argv[1:]
import verify


def save(name, data):
    payload = json.dumps(data, sort_keys=True, separators=(',', ':')).encode()
    (out / name).write_bytes(gzip.compress(payload, mtime=0) if name.endswith('.gz') else payload + b'\n')


def statuses(rows):
    return {(url, name, status) for url, (_, _, checks) in rows.items() for name, status in checks}


def state(rows):
    return {(url, row[0], row[1]) for url, row in rows.items()}


def at(rows, url, name):
    return sorted({result for key, result in rows[url][2] if key == name})


for cohort in ('main', 'supplement'):
    candidate = verify.load(out / f'outcomes-{cohort}.json.gz')
    references = {'previous': verify.load(previous / f'outcomes-{cohort}.json.gz'), **{
        label: verify.load(frozen / f'outcomes-{label}-{cohort}.json.gz') for label in ('frozen', 'chrome')}}
    for label, reference in references.items():
        assert candidate.keys() == reference.keys()
        left = verify.stats(candidate) | {'status': statuses(candidate), 'pageState': state(candidate)}
        right = verify.stats(reference) | {'status': statuses(reference), 'pageState': state(reference)}
        diff = {key: {'candidateOnly': sorted(left[key] - right[key]), 'referenceOnly': sorted(right[key] - left[key])} for key in left}
        save(f'all-{cohort}-vs-{label}.json.gz', diff)
        print(cohort, label, {key: [len(items) for items in value.values()] for key, value in diff.items()})
        if label == 'previous':
            controls = set(diff['pages']['referenceOnly']) | {row[0] for key in ('passes','observed') for row in diff[key]['referenceOnly']}
            controls.update(url for url, status, harness in diff['pageState']['candidateOnly'] if [status, harness] != ['complete', 0])
            save(f'repeat-{cohort}-urls.json', sorted(controls))
    if cohort == 'main':
        hdr = verify.load(focused / 'round7-css-color-hdr/differential.json')
        changes = [(url, name, 'HDR ' + direction) for direction in ('gained', 'lost') for url, name, _ in hdr[direction]]
        geometry = focused / 'round7-overflow-geometry'
        urls = set(verify.load(geometry / 'urls.json'))
        base, fixed = [verify.collect([geometry / (label + '.json.gz')], urls, False) for label in ('base', 'candidate')]
        gains = verify.stats(fixed)['passes'] - verify.stats(base)['passes']
        assert len(gains) == 12 and len(changes) == 102
        changes += [(url, name, 'geometry gained') for url, name in sorted(gains)]
        mapped = [{'url': url, 'name': name, 'focused': kind, **{label: at(rows, url, name) for label, rows in (references | {'candidate': candidate}).items()}} for url, name, kind in changes]
        save('focused-map.json.gz', mapped)
        print(collections.Counter((row['focused'], tuple(row['previous']), tuple(row['candidate']), tuple(row['chrome'])) for row in mapped))
