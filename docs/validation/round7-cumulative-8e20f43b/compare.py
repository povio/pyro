import gzip
import json
import sys
from collections import Counter
from pathlib import Path


output, previous, frozen, focus = map(Path, sys.argv[1:])
del sys.argv[1:]
sys.path.insert(0, str(frozen))
import verify


def save(filename, data):
    payload = json.dumps(data, sort_keys=True, separators=(',', ':')).encode()
    (output / filename).write_bytes(gzip.compress(payload, mtime=0))


def metrics(rows):
    return dict(verify.stats(rows), status={(url, name, status) for url, (_, _, tests) in rows.items() for name, status in tests},
                pageState={(url, row[0], row[1]) for url, row in rows.items()})


def at(rows, key):
    url, name = key
    return sorted({result for label, result in rows[url][2] if label == name})


main, supplement, withheld = [verify.load(frozen / (name + '.json')) for name in ('main', 'supplement', 'withheld')]
assert [len(main), len(supplement), len(withheld), len(set(main + supplement + withheld))] == [8082, 41, 39, 8162]
for cohort, urls in [('main', main), ('supplement', supplement)]:
    stems = [f'{index:02d}' for index in range(24)] if cohort == 'main' else [cohort]
    files = [output / f'raw-{stem}.json' for stem in stems]
    candidate = verify.collect(files, set(urls), cohort == 'supplement')
    for path, stem in zip(files, stems):
        raw = verify.load(path)
        manifest = verify.load(output / f'urls-{stem}.json') if cohort == 'main' else urls
        assert Counter(raw['plan']) == Counter(manifest)
        assert raw['jobs'] == 2 and raw['mode'] == 'full' and not raw['reuseContext'] and raw['dirs'] == ['css']
    save(f'outcomes-{cohort}.json.gz', candidate)
    references = dict(previous=verify.load(previous / f'outcomes-{cohort}.json.gz'))
    references.update({label: verify.load(frozen / f'outcomes-{label}-{cohort}.json.gz') for label in ('frozen', 'chrome')})
    left = metrics(candidate)
    print(cohort, 'candidate', {key: len(items) for key, items in left.items()}, Counter((row[0],row[1]) for row in candidate.values()))
    for label, rows in references.items():
        assert candidate.keys() == rows.keys()
        right = metrics(rows)
        diff = {key: dict(candidateOnly=sorted(items-right[key]), referenceOnly=sorted(right[key]-items)) for key, items in left.items()}
        save(f'all-{cohort}-vs-{label}.json.gz', diff)
        print(cohort, label, {key: [len(direction) for direction in pair.values()] for key, pair in diff.items()})
        if label == 'previous':
            lost = right['passes'] - left['passes']
            coverage = sorted(lost - left['observed'])
            observed_losses = sorted(lost & left['observed'])
            save(f'losses-{cohort}.json.gz', dict(unobserved=coverage, observed=observed_losses))
            print('Observed and missing named losses:', len(observed_losses), len(coverage))
    if cohort == 'main':
        focus_urls = set(verify.load(focus.parent / 'round7-overflow-geometry/urls.json'))
        before, after = [verify.collect([focus / f'{label}.json.gz'], focus_urls, False) for label in ('base', 'candidate')]
        gains = verify.stats(after)['passes'] - verify.stats(before)['passes']
        assert len(gains) == 24
        mapped = [dict(key=key, **{label: at(rows, key) for label, rows in dict(references, candidate=candidate).items()}) for key in sorted(gains)]
        save('focus-24.json.gz', mapped)
        print('Focus previous/candidate/frozen/chrome:', Counter(tuple(tuple(row[label]) for label in ('previous','candidate','frozen','chrome')) for row in mapped))
        historical = verify.load(previous / 'repeats-main.json.gz')['passes']
        assert len(historical) == 3 and all(row['reproducedBoth'] for row in historical)
        inherited = [dict(key=row['item'], priorControls=row['controls'], **{label: at(rows, row['item']) for label, rows in dict(references, candidate=candidate).items()}) for row in historical]
        save('prior-hdr-3.json.gz', inherited)
        print('Prior HDR previous/candidate/frozen/chrome:', Counter(tuple(tuple(row[label]) for label in ('previous','candidate','frozen','chrome')) for row in inherited))
