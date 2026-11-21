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
batches = {(): [(5, 4), (7, 5), (8, 7), (9, 8)], ('--batch10',): [(10, 9)], ('--batch11',): [(11, 10)], ('--batch12',): [(12, 11)], ('--batch13',): [(13, 12)], ('--batch14',): [(14, 13)], ('--batch16',): [(16, 14)]}[tuple(sys.argv[1:])]
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
    if batch >= 10:
        assert source == {10: 'c10efae54a7ce4c25be1d8bab090fcc34cab6e7b', 11: '1ee620d451c093a2ddce8766172970ed0ce8275b', 12: '8695db84513be44b2aedc700e3e042ee9354b2b4', 13: '0f4857a4dfd126e96ed9431ec115644c49630d16', 14: '8627d988ed60b15992282350b5c17d97cb39d9e7', 16: '55bca2409fdf6ac3695558827066ea4f8f3f639e'}[batch]
        prior = root / f'batch{previous}'
        assert subprocess.check_output(['git', 'rev-parse', source+('^' if batch in (10, 13, 14) else '^^')]) == (prior / 'source.txt').read_bytes()
        assert verify['digest'](cache / prior.name / 'candidate')['sha256'] == (prior / 'sha256.txt').read_text().split()[0]
        shutil.copyfile(raw / 'build.log', out / 'build.log')
    timings = {}
    for cohort in ('main', 'supplement'):
        sharded = batch >= 9 and cohort == 'main'
        reports = [raw / f'main-{index}.json' for index in range(16)] if sharded else [raw / f'{cohort}.json']
        urls = set(cohorts[cohort])
        outcomes = collect(reports, urls, cohort == 'supplement')
        summary = {'urls': len(urls), 'reports': {}}
        for index, report in enumerate(reports):
            data = read(report)
            if batch == 16:
                assert data['plan'] == read(cache / prior.name / report.name)['plan']
            assert data['bin'] == str(binary) and data['jobs'] == 2 and not data['reuseContext']
            assert data['dirs'] == ['css'] and data['discovered'] == len(data['plan'])
            entry = {**verify['digest'](report), **{key: data[key] for key in ('wallMs','restarts','discovered','jobs')}}
            if sharded:
                plan = cache / 'batch9' / f'urls-{index}.json'
                if batch >= 10:
                    assert plan.read_bytes() == gzip.decompress((root / 'batch9' / (plan.name+'.gz')).read_bytes())
                assert Counter(data['plan']) == Counter(read(plan))
                if batch == 14:
                    assert Counter(data['plan']) == Counter(read(cache / 'batch13' / report.name)['plan'])
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
            if batch >= 11 and label != 'batch1':
                if label == 'frozen':
                    originals = [cache / 'frozen-evidence' / f'frozen-{cohort}.json']
                elif label == 'chrome':
                    original_dir = cache.parent / 'css-th-e4ca26'
                    originals = [original_dir / f'strict-chrome-{i}.json' for i in range(16)] if sharded else [original_dir / 'supplement-chrome-cert.json']
                else:
                    originals = [cache / label / report.name for report in reports]
                assert reference == collect(originals, urls, cohort == 'supplement')
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
    if batch == 16:
        repeats = out / 'matched-repeats'
        urls = set(read(repeats / 'urls.json'))
        full = {label: read(root / f'batch{label}/outcomes-main.json.gz') for label in (14, 16)}
        assert len(urls) == 9 and urls == {url for url in full[14] if full[14][url][:2] != full[16][url][:2]}
        matched = {}
        for label in ('14a', '16a', '16b', '14b'):
            report = repeats / (label + '.json')
            data = read(report)
            assert data['bin'] == str(cache / ('batch'+label[:2]) / 'candidate') and data['jobs'] == 2
            assert not data['reuseContext'] and data['restarts'] == data['watchdogKills'] == 0
            matched[label] = collect([report], urls, False)
            assert matched[label] == {url:full[14][url] for url in urls}
    paths = sorted(path for path in out.rglob('*') if path.is_file() and path.name != 'SHA256SUMS')
    (out / 'SHA256SUMS').write_text(''.join(f'{hashlib.sha256(path.read_bytes()).hexdigest()}  {path.relative_to(out)}\n' for path in paths))
