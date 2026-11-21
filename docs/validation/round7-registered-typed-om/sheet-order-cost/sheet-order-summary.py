import json
import statistics
import sys
from pathlib import Path

arms = [json.loads(line) for line in Path(sys.argv[1]).read_text().splitlines()]
assert len(arms) == 8
for name in arms[0]['samples']:
    pairs = []
    by_binary = {}
    for arm in arms:
        values = arm['samples'][name]
        by_binary.setdefault(Path(arm['binary']).name, []).extend(values)
    stats = []
    for binary, rows in by_binary.items():
        stats.append(dict(binary=binary, ms=statistics.median(x['elapsedMs'] for x in rows),
                          wall=statistics.median(x['wallMs'] for x in rows), cpu=statistics.mean(x['cpuMs'] for x in rows),
                          mismatches=sum(x['mismatches'] for x in rows), reads=sum(x['reads'] for x in rows)))
    for first,second in zip(arms[::2],arms[1::2]):
        pair = {Path(arm['binary']).name:statistics.median(x['elapsedMs'] for x in arm['samples'][name])
                for arm in (first,second)}
        pairs.append(round(100*(pair[stats[1]['binary']]/pair[stats[0]['binary']]-1),1))
    print(json.dumps(dict(fixture=name, arms=stats, pairChangePct=pairs)))
