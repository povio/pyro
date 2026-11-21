import gzip
import json
import sys
from collections import Counter
from pathlib import Path


output, previous, frozen, focus = map(Path, sys.argv[1:])
del sys.argv[1:]
sys.path.insert(0, str(frozen))
import verify


def save(name, value):
    data = json.dumps(value, sort_keys=True, separators=(',', ':')).encode()
    (output / name).write_bytes(gzip.compress(data, mtime=0))


def metrics(rows):
    return dict(verify.stats(rows), pageState={(url, state, harness) for url, (state, harness, _) in rows.items()},
                status={(url, name, status) for url, (_, _, tests) in rows.items() for name, status in tests})


def occurrences(rows):
    result = {}
    for url, (_, _, tests) in rows.items():
        occurrence = Counter()
        for name, status in tests:
            occurrence[name] += 1
            result[url, name, occurrence[name]] = status
    return result


def compare_focus(sources):
    selected = set(verify.load(focus / 'spm-urls.json'))
    focused = verify.load(focus / 'final-named-with-chrome.json.gz')['observations']
    expected = verify.load(focus / 'final-differential.json')
    previous_keys = set(sources['previous'])
    assert selected <= previous_keys and len(selected) == 43
    outcomes = {label: occurrences({url: data[url] for url in selected}) for label, data in sources.items()}
    records = []
    for row in focused:
        key = (row['url'], row['name'], row['occurrence'])
        records.append(dict(key=key, base=row['base'], head=row['head'], focusChrome=row['chrome158'],
                            **{label: data.get(key) for label, data in outcomes.items()}))
    assert len(records) == len({row['key'] for row in records}) == expected['names'] == 3142
    focus_gains = {tuple(key) for key in expected['gained']}
    assert focus_gains == {row['key'] for row in records if row['base'] != 'Pass' and row['head'] == 'Pass'}
    save('focus-occurrences.json.gz', records)
    old, new = (metrics(sources[label]) for label in ('previous', 'candidate'))
    named = {key[:2] for key in focus_gains}
    pages = set(expected['gainedPages'])
    rest = {}
    for metric, intended in [('passes', named), ('pages', pages)]:
        gained, lost = new[metric]-old[metric], old[metric]-new[metric]
        rest[metric] = dict(focused=sorted(gained & intended), missingFocus=sorted(intended-gained),
                            otherGains=sorted(gained-intended), losses=sorted(lost))
    save('attribution.json.gz', rest)
    print('attribution', {metric: {key:len(items) for key,items in group.items()} for metric,group in rest.items()})
    print('focus mismatch counts', {key:sum(row[left]!=row[right] for row in records) for key,left,right in
          [('base','previous','base'),('head','candidate','head'),('chrome','chrome','focusChrome')]})


main, supplement, withheld = [verify.load(frozen / (name+'.json')) for name in ('main','supplement','withheld')]
assert [len(main),len(supplement),len(withheld),len(set(main+supplement+withheld))] == [8082,41,39,8162]
for cohort, urls in [('main', main), ('supplement', supplement)]:
    stems = [f'{index:02}' for index in range(24)] if cohort == 'main' else [cohort]
    files = [output / f'raw-{stem}.json' for stem in stems]
    candidate = verify.collect(files, set(urls), cohort == 'supplement')
    for stem, path in zip(stems, files):
        raw = verify.load(path)
        manifest = verify.load(output / f'urls-{stem}.json') if cohort == 'main' else urls
        assert Counter(raw['plan']) == Counter(manifest)
        assert raw['jobs'] == 2 and raw['mode'] == 'full' and not raw['reuseContext'] and raw['dirs'] == ['css']
        assert Path(raw['bin']).name == 'candidate-pyro'
    save(f'outcomes-{cohort}.json.gz', candidate)
    refs = {'previous':verify.load(previous / f'outcomes-{cohort}.json.gz')}
    refs.update({label:verify.load(frozen / f'outcomes-{label}-{cohort}.json.gz') for label in ('frozen','chrome')})
    left = metrics(candidate)
    print(cohort, 'candidate', {key:len(items) for key,items in left.items()})
    for label, rows in refs.items():
        assert rows.keys() == candidate.keys()
        right = metrics(rows)
        diff = {key:dict(candidateOnly=sorted(items-right[key]), referenceOnly=sorted(right[key]-items)) for key,items in left.items()}
        save(f'all-{cohort}-vs-{label}.json.gz', diff)
        print(cohort, label, {key:[len(values) for values in pair.values()] for key,pair in diff.items()})
        if label == 'previous':
            lost = right['passes']-left['passes']
            save(f'losses-{cohort}.json.gz', dict(observed=sorted(lost & left['observed']), unobserved=sorted(lost-left['observed'])))
    if cohort == 'main':
        compare_focus(dict(refs, candidate=candidate))
        inherited = verify.load(previous / 'prior-hdr-3.json.gz')
        missing = verify.load(previous / 'all-main-vs-previous.json.gz')['observed']['referenceOnly']
        controls = verify.load(previous / 'controls.json')
        assert len(inherited) == 3 and len(missing) == 1
        retained = []
        for key in [row['key'] for row in inherited] + missing:
            row = dict(key=key, kind='previously missing failure' if key in missing else 'inherited HDR failure')
            for label, data in dict(refs, candidate=candidate).items():
                page = data[key[0]]
                row[label] = [*page[:2], [status for name,status in page[2] if name == key[1]]]
            row['earlierControls'] = next((item['ABBA'] for item in controls if item['key']==key), None) or next(item['priorControls'] for item in inherited if item['key']==key)
            retained.append(row)
        save('inherited.json.gz', retained)
        print('inherited', [(row['kind'],row['previous'],row['candidate']) for row in retained])
