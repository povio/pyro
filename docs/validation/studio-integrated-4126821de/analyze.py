import gzip
import json
import sys
from collections import Counter
from pathlib import Path

out = Path(sys.argv.pop(1)).resolve()
frozen = out.parent / 'frozen-evidence'
sys.dont_write_bytecode = True
sys.path.insert(0, str(frozen))
import verify


def save(name, value):
    data = json.dumps(value, sort_keys=True, separators=(',', ':')).encode()
    (out / name).write_bytes(gzip.compress(data, mtime=0) if name.endswith('.gz') else data + b'\n')


for cohort in ('main', 'supplement'):
    sup = cohort == 'supplement'
    files = [out/'raw-supplement.json'] if sup else sorted(out.glob('raw-[0-9][0-9].json'))
    urls = set(verify.load(frozen/f'{cohort}.json'))
    assert not urls.intersection(verify.load(frozen/'withheld.json'))
    results = verify.collect(files, urls, sup)
    summary = {}
    save(f'outcomes-{cohort}.json.gz', results)
    candidate = verify.stats(results)
    summary['candidate'] = {key:len(values) for key,values in candidate.items()}
    summary['states'] = dict(Counter(f'{row[0]}:{row[1]}' for row in results.values()))
    for file in files:
        raw = verify.load(file)
        assert raw['jobs']==2 and raw['mode']=='full' and not raw['reuseContext'] and raw['dirs']==['css']
        manifest = frozen/'supplement.json' if sup else out/file.name.replace('raw-', 'urls-')
        assert set(raw['plan']) == set(verify.load(manifest))
    for label in ('frozen','chrome'):
        reference = verify.load(frozen/f'outcomes-{label}-{cohort}.json.gz')
        stats = verify.stats(reference)
        diff = {key:{'candidateOnly':sorted(values-stats[key]), 'referenceOnly':sorted(stats[key]-values)} for key,values in candidate.items()}
        save(f'difference-{cohort}-vs-{label}.json.gz', diff)
        summary[label] = {key:{direction:len(items) for direction,items in pair.items()} for key,pair in diff.items()}
        if label == 'frozen':
            controls = set(diff['pages']['referenceOnly']) | {item[0] for key in ('passes','observed') for item in diff[key]['referenceOnly']}
            for url,row in results.items():
                if row[:2] != reference[url][:2] and row[:2] != ['complete',0]: controls.add(url)
            save(f'controls-{cohort}.json', sorted(controls))
    save(f'summary-{cohort}.json',summary)
    print(cohort,summary)
