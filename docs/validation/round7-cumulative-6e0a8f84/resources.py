import json
import re
import statistics
import sys
from collections import Counter
from pathlib import Path


output = Path(sys.argv[1])
reports = [json.loads(path.read_bytes()) for path in sorted(output.glob('raw-*.json'))]
assert len(reports) == 25
samples = (output / 'resources.log').read_text()
idle = [float(value) for value in re.findall(r'([\d.]+)% idle', samples)]
logs = [path.read_text() for path in sorted(output.glob('run-*.log'))]
print(json.dumps(dict(
    startEpoch=int((output / 'primary-start').read_text()),
    endEpoch=int((output / 'primary-end').read_text()),
    sampledHostBusyPct=dict(count=len(idle), mean=round(100-statistics.mean(idle),2),
                            minimum=round(100-max(idle),2), maximum=round(100-min(idle),2)),
    totalCpuSeconds={key:round(sum(report[key] for report in reports),3) for key in ('browserCpuS','runnerCpuS','serverCpuS')},
    peakCommandRssBytes=max(int(re.search(r'(\d+)  maximum resident set size', log)[1]) for log in logs),
    restarts=sum(report['restarts'] for report in reports), watchdogKills=sum(report['watchdogKills'] for report in reports),
    mainShardMs=[min(report['wallMs'] for report in reports[:-1]),max(report['wallMs'] for report in reports[:-1])],
    supplementMs=reports[-1]['wallMs'], states={cohort:Counter(f"{row['status']}:{row.get('harnessStatus')}" for report in group for row in report['results']) for cohort,group in [('main',reports[:-1]),('supplement',reports[-1:])]},
), indent=2))
