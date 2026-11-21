import argparse
import json
import re
from collections import Counter, defaultdict
from pathlib import Path
from urllib.parse import parse_qs, urlsplit

root = Path(__file__).resolve().parent
arg = argparse.ArgumentParser()
arg.add_argument('run')
label = arg.parse_args().run
events = [json.loads(s) for s in (root / f'events-{label}.jsonl').read_text().splitlines() if s]
receipt = json.loads((root / f'{label}.json').read_text())
native = (root / f'native-{label}.log').read_text().splitlines()
steps = defaultdict(list)
exceptions = []
for line in native:
    m = re.match(r'RPTRACE (\d+) ([^ ]+)(?: (.*))?$', line)
    if m:
        steps[int(m[1])].append((m[2], m[3]))
    if line.startswith('RPEXCEPTION '):
        exceptions.append(line)
def echo_key(url):
    if not url: return None
    u = urlsplit(url)
    if not u.path.endswith('/subresource/document.py'): return None
    return parse_qs(u.query).get('key', [None])[0]
views = defaultdict(lambda: {'stages': Counter(), 'frames': set(), 'results': set()})
pages = defaultdict(set)
for e in events:
    k = echo_key(e.get('url')) or echo_key(e.get('dst'))
    if not k: continue
    v = views[k]
    pages[e['page']].add(k)
    stage = e.get('event') or e['stage']
    if stage == 'message':
        stage += '-' + str(e.get('type'))
        if e.get('type') == 'result':
            v['results'].add((e.get('dst'), e.get('source')))
    if stage == 'load':
        v['frames'].add((e.get('dst'), e.get('grandparentSlot'), e.get('resultDefined')))
    v['stages'][stage] += 1
for k, v in views.items():
    v['frames'] = sorted(v['frames'],key=str)
    v['results'] = sorted(v['results'],key=str)
    v['nativeSourceMessages'] = {}
    for trace, row in steps.items():
        entry = next((s for stage,s in row if stage == 'entry'), None)
        if entry and k in entry.partition(' target=')[0]:
            v['nativeSourceMessages'][trace] = row
report={'source':'81c195367a245cd91177e28330fef28f264b660b', 'label':label, 'restarts':receipt['restarts'], 'watchdogKills':receipt['watchdogKills'],
        'pages': {x['url']:{'harnessStatus':x.get('harnessStatus'), 'status':x['status'], 'names':dict(Counter(t['status'] for t in x['subtests'])),
                              'echoKeys':sorted(pages[x['url']])} for x in receipt['results']},
        'echoes':views,'nativeExceptions':exceptions,'pageExceptions':[x for x in events if x['stage']=='exception']}
(root/f'analysis-{label}.json').write_text(json.dumps(report,indent=2)+'\n')
print('pages',len(receipt['results']),'echoes',len(views),'native entries',len(steps),'uncaught exceptions',len(exceptions))
for k,v in views.items():
    if not v['results'] or not v['nativeSourceMessages']:
        print('unmatched',k,dict(v['stages']),v['nativeSourceMessages'])
