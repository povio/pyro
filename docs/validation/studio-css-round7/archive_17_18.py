import hashlib
import re
import runpy
import shutil
import stat
import subprocess
import sys
from collections import Counter
from datetime import datetime
from pathlib import Path
from zoneinfo import ZoneInfo


root = Path(__file__).resolve().parent
cache = Path(sys.argv[1]).resolve()
helpers = runpy.run_path(str(root / 'batch1/compare.py'))
read, save, sets = (helpers[key] for key in ('read', 'save', 'sets'))
cohorts = {name: read(root / f'frozen/{name}.json.gz') for name in ('main', 'supplement', 'withheld')}
assert [len(urls) for urls in cohorts.values()] == [8082, 41, 39]
assert len(set().union(*cohorts.values())) == 8162
sources = {17: '063c60bb14b4e9c744a75d29c1748522b57e49bb', 18: '83bb4bdbbfe7d5b3fe53f68facbe5986f1ca2192'}


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def commit(rev):
    return subprocess.check_output(['git', 'rev-parse', rev], text=True).strip()


for batch, source in sources.items():
    label = f'batch{batch}'
    raw = cache / label
    out = root / label
    out.mkdir(exist_ok=True)
    binary = raw / 'candidate'
    assert (raw / 'source.txt').read_text().strip() == commit(source+'^{commit}') == source
    assert commit(source+('^^' if batch == 17 else '^')) == (cache / f'batch{16 if batch == 17 else 17}/source.txt').read_text().strip()
    assert (raw / 'sha256.txt').read_text().split() == [digest(binary), str(binary)]
    assert stat.S_IMODE(binary.stat().st_mode) == 0o555
    for name in ('source.txt', 'sha256.txt', 'build.log'):
        shutil.copyfile(raw / name, out / name)
    timing = {}
    for cohort in ('main', 'supplement'):
        names = [f'main-{index}' for index in range(16)] if cohort == 'main' else ['supplement']
        comparisons = ['batch14', 'batch16'] + (['batch17'] if batch == 18 else [])
        outcomes = {key: {} for key in [label, *comparisons]}
        reports = {}
        for index, name in enumerate(names):
            baseline = None
            for key in [*comparisons, label]:
                path = cache / key / (name+'.json')
                data = read(path)
                plan = data['plan']
                expected = read(root / f'batch9/urls-{index}.json.gz') if cohort == 'main' else cohorts[cohort]
                assert Counter(plan) == Counter(expected) == Counter(row['url'] for row in data['results'])
                assert baseline is None or baseline == plan
                baseline = plan
                assert data['label'] == key and data['bin'] == str(cache / key / 'candidate')
                assert data['wptRevision'] == 'a421f2fea5eb7f91e0db399e44b0fb0779ee1603' and data['dirs'] == ['css']
                assert data['testdriver'] and data['server'] == 'wptserve' and data['restricted'] and data['kind'] == 'testharness'
                assert data['workerGlobals'] == (cohort == 'supplement') and data['jobs'] == 2 and not data['reuseContext'] and not data['skipped'] and data['discovered'] == len(plan)
                for row in data['results']:
                    assert row['url'] not in outcomes[key]
                    outcomes[key][row['url']] = [row['status'], row.get('harnessStatus'), [[sub['name'], sub['status']] for sub in row['subtests'] if sub['name'] != 'no test suite completion']]
            save(out / (name+'-plan.json.gz'), plan)
            reports[name] = {key:data[key] for key in ('browserCpuS','runnerCpuS','serverCpuS','wallMs','watchdogKills','restarts','discovered')}
            reports[name]['rawSha256'] = digest(path)
            log = path.with_suffix('.log')
            shutil.copyfile(log, out / log.name)
            timing[name] = [log.stat().st_birthtime, path.stat().st_mtime]
        assert outcomes['batch14'] == read(root / f'batch14/outcomes-{cohort}.json.gz')
        for key in ('frozen', 'chrome'):
            outcomes[key] = read(root / f'frozen/outcomes-{key}-{cohort}.json.gz')
        assert all(set(rows) == set(cohorts[cohort]) for rows in outcomes.values())
        save(out / f'outcomes-{cohort}.json.gz', outcomes[label])
        candidate = sets(outcomes.pop(label))
        counts = {'candidate': {key:len(items) for key,items in candidate.items()}, 'reports':reports}
        for reference, rows in outcomes.items():
            other = sets(rows)
            diff = {key: {'candidateOnly':sorted(candidate.get(key,set())-other.get(key,set())), 'referenceOnly':sorted(other.get(key,set())-candidate.get(key,set()))} for key in candidate.keys() | other.keys()}
            save(out / f'difference-{cohort}-vs-{reference}.json.gz', diff)
            counts[reference] = {key:{side:len(items) for side,items in sides.items()} for key,sides in diff.items()}
        save(out / f'summary-{cohort}.json',counts)
    start = min(pair[0] for pair in timing.values())
    end = max(pair[1] for pair in timing.values())
    samples = []
    for log in sorted(cache.glob('host-*.log')):
        for stamp,load,idle in re.findall(r'(\d{4}/\d\d/\d\d \d\d:\d\d:\d\d)\nLoad Avg: ([^\n]+)\nCPU usage: [^\n]+?([\d.]+)% idle',log.read_text()):
            when = datetime.strptime(stamp,'%Y/%m/%d %H:%M:%S').replace(tzinfo=ZoneInfo('Europe/Ljubljana')).timestamp()
            if start <= when <= end:
                samples.append([stamp,load,round(100-float(idle),2)])
    save(out / 'timing.json',dict(elapsedSeconds=end-start,logBirthReportMtime=timing,hostLoadBusyPercent=samples,binaryBytes=binary.stat().st_size))
    (out/'SHA256SUMS').write_text(''.join(f'{digest(path)}  {path.name}\n' for path in sorted(out.iterdir()) if path.name != 'SHA256SUMS'))
