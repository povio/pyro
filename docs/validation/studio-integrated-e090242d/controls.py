import gzip
import json
import sys
from pathlib import Path


out, previous, frozen = map(Path, sys.argv[1:])
sys.path.insert(0, str(frozen))
del sys.argv[1:]
import verify


for cohort in ('main', 'supplement'):
    urls = set(verify.load(out / f'repeat-{cohort}-urls.json'))
    if not urls:
        continue
    runs = [verify.collect([out / f'repeat-{cohort}-{label}.json'], urls, cohort == 'supplement') for label in ('previous-1', 'candidate-1', 'candidate-2', 'previous-2')]
    before = verify.load(previous / f'outcomes-{cohort}.json.gz')
    after = verify.load(out / f'outcomes-{cohort}.json.gz')
    primary = verify.load(out / f'all-{cohort}-vs-previous.json.gz')
    report = {}
    for metric in ('passes','pages','observed','pageState'):
        losses = primary[metric]['referenceOnly']
        findings = []
        for item in losses:
            url = item if metric == 'pages' else item[0]
            if url not in urls:
                continue
            def value(data):
                if metric in ('passes','observed'):
                    return sorted({status for name, status in data[url][2] if name == item[1]})
                if metric == 'pages':
                    return url in verify.stats({url: data[url]})['pages']
                return data[url][:2]
            values = [value(data) for data in [before, after, *runs]]
            findings.append({'item':item, 'primary':values[:2], 'controls':values[2:], 'reproducedBoth':values[2:] == [values[0],values[1],values[1],values[0]]})
        report[metric] = findings
        print(cohort, metric, sum(row['reproducedBoth'] for row in findings), '/', len(findings))
    (out / f'repeats-{cohort}.json.gz').write_bytes(gzip.compress(json.dumps(report,sort_keys=True).encode(),mtime=0))
