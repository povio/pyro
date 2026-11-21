import gzip
import hashlib
import json
import re
import runpy
import shutil
import statistics
import subprocess
import sys
from collections import Counter
from datetime import datetime
from pathlib import Path
from zoneinfo import ZoneInfo


root = Path(__file__).resolve().parent
cache = Path(sys.argv.pop(1)).resolve()
batches = [(10, 9)] if sys.argv[1:] == ['--batch10'] else [(5, 4), (7, 5), (8, 7), (9, 8)]
assert sys.argv[1:] in ([], ['--batch10'])
del sys.argv[1:]
verifier = cache / 'frozen-evidence/verify.py'
assert verifier.read_bytes() == (root / 'frozen/verify.py').read_bytes()
verify = runpy.run_path(str(verifier))
read, collect = verify['load'], verify['collect']
helpers = runpy.run_path(str(root / 'batch1/compare.py'))
save, sets = helpers['save'], helpers['sets']
cohorts = {name: read(root / f'frozen/{name}.json.gz') for name in ('main', 'supplement', 'withheld')}
assert list(map(len, cohorts.values())) == [8082, 41, 39]
assert len(set().union(*cohorts.values())) == 8162
zone = ZoneInfo('Europe/Ljubljana')
samples = []
for log in sorted(cache.glob('host-*.log')):
    text = log.read_text()
    for stamp, load, idle in re.findall(r'(\d{4}/\d\d/\d\d \d\d:\d\d:\d\d)\nLoad Avg: ([^\n]+)\nCPU usage: [^\n]+?([\d.]+)% idle', text):
        instant = datetime.strptime(stamp, '%Y/%m/%d %H:%M:%S').replace(tzinfo=zone)
        samples.append((instant.timestamp(), log.name, list(map(float, load.split(', '))), round(100-float(idle), 2)))

for batch, previous in batches:
    raw = cache / f'batch{batch}'
    out = root / raw.name
    out.mkdir(exist_ok=True)
    for name in ('source.txt', 'sha256.txt'):
        shutil.copyfile(raw / name, out / name)
    binary = raw / 'candidate'
    receipt, path = (raw / 'sha256.txt').read_text().split()
    assert path == str(binary) and verify['digest'](binary)['sha256'] == receipt
    source = (raw / 'source.txt').read_text().strip()
    assert subprocess.check_output(['git', 'rev-parse', source+'^{commit}'], text=True).strip() == source
    if batch == 10:
        assert source == 'c10efae54a7ce4c25be1d8bab090fcc34cab6e7b'
        prior = root / 'batch9'
        assert subprocess.check_output(['git', 'rev-parse', source+'^']) == (prior / 'source.txt').read_bytes()
        assert verify['digest'](cache / 'batch9/candidate')['sha256'] == (prior / 'sha256.txt').read_text().split()[0]
        shutil.copyfile(raw / 'build.log', out / 'build.log')
    timings = {}
    for cohort in ('main', 'supplement'):
        sharded = batch in (9, 10) and cohort == 'main'
        reports = [raw / f'main-{index}.json' for index in range(16)] if sharded else [raw / f'{cohort}.json']
        urls = set(cohorts[cohort])
        outcomes = collect(reports, urls, cohort == 'supplement')
        summary = {'urls': len(urls), 'reports': {}}
        for index, report in enumerate(reports):
            data = read(report)
            assert data['bin'] == str(binary) and data['jobs'] == 2 and not data['reuseContext']
            assert data['dirs'] == ['css'] and data['discovered'] == len(data['plan'])
            entry = {**verify['digest'](report), **{key: data[key] for key in ('wallMs','restarts','discovered','jobs')}}
            if sharded:
                plan = cache / 'batch9' / f'urls-{index}.json'
                if batch == 10:
                    assert plan.read_bytes() == gzip.decompress((root / 'batch9' / (plan.name+'.gz')).read_bytes())
                assert Counter(data['plan']) == Counter(read(plan))
                (out / (plan.name+'.gz')).write_bytes(gzip.compress(plan.read_bytes(), mtime=0))
                entry['planSha256'] = verify['digest'](plan)['sha256']
            summary['reports'][report.name] = entry
            log = report.with_suffix('.log')
            shutil.copyfile(log, out / log.name)
            timings[report.name] = [log.stat().st_birthtime, report.stat().st_mtime, data['wallMs']]
        save(out / f'outcomes-{cohort}.json.gz', outcomes)
        candidate = sets(outcomes)
        summary['candidate'] = {key:len(values) for key, values in candidate.items()}
        for label in ('frozen','chrome','batch1',f'batch{previous}'):
            if label in ('frozen','chrome'):
                filename = f'outcomes-{label}-{cohort}.json.gz'
                reference = read(root / 'frozen' / filename)
                assert reference == read(cache / 'frozen-evidence' / filename)
            else:
                reference = read(root / label / f'outcomes-{cohort}.json.gz')
            assert reference.keys() == outcomes.keys()
            other = sets(reference)
            delta = {}
            for key in candidate.keys() | other.keys():
                left, right = candidate.get(key,set()), other.get(key,set())
                delta[key] = dict(candidateOnly=sorted(left-right), referenceOnly=sorted(right-left))
            summary['vs-'+label] = {key:{side:len(items) for side,items in value.items()} for key,value in delta.items()}
            save(out / f'difference-{cohort}-vs-{label}.json.gz', delta)
        save(out / f'summary-{cohort}.json', summary)
    start = min(value[0] for value in timings.values())
    end = max(value[1] for value in timings.values())
    during = [sample for sample in samples if start <= sample[0] <= end]
    busy = [sample[-1] for sample in during]
    save(out / 'timing.json', dict(logBirthReportMtimeWallMs=timings, elapsedSeconds=end-start, hostSamples=during,
                                   busyPercent=dict(min=min(busy), median=statistics.median(busy), mean=statistics.mean(busy), max=max(busy))))
    if batch == 7:
        repeats = raw / 'navigation-repeats'
        urls = set(read(repeats / 'urls.json'))
        assert len(urls) == 3 and urls <= set(cohorts['main'])
        first = None
        for head in ('batch5','batch7'):
            for rep in (1,2):
                report = repeats / f'{head}-{rep}.json'
                data = read(report)
                assert data['bin'] == str(cache / head / 'candidate') and data['jobs'] == 2
                matched = collect([report], urls, False)
                if first is None:
                    first = matched
                assert matched == first
        shutil.copytree(repeats, out / repeats.name, dirs_exist_ok=True)
    paths = sorted(path for path in out.rglob('*') if path.is_file() and path.name != 'SHA256SUMS')
    (out / 'SHA256SUMS').write_text(''.join(f'{hashlib.sha256(path.read_bytes()).hexdigest()}  {path.relative_to(out)}\n' for path in paths))
