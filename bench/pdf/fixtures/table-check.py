#!/usr/bin/env python3
"""Compare a probe of generators/fx-tcell.mjs with Chrome 156's markers: words on the same pages at the same x and y, 0.5 pt.

usage: table-check.py chrome-156/tcell-x.json probe.json [-v]
       table-check.py --extract chrome-probe.json chrome-156/tcell-x.json
"""
import json
import sys


def extract(run):
    return {'pages': run['pages'], 'words': [[[w['t'], round(w['x'], 2), round(w['y1'], 2)] for w in page] for page in run['words']]}


if sys.argv[1] == '--extract':
    probe = json.load(open(sys.argv[2]))
    json.dump({k: extract(v) for k, v in probe.items()}, open(sys.argv[3], 'w'), separators=(',', ':'))
    sys.exit()

chrome, lp = json.load(open(sys.argv[1])), json.load(open(sys.argv[2]))
verbose = '-v' in sys.argv
groups, out = {}, []
for name, want in chrome.items():
    got = extract(lp[name])
    a = {w[0]: w for page in want['words'] for w in page}
    b = {w[0]: w for page in got['words'] for w in page}
    far = max((abs(a[t][1] - b[t][1]) if t in b else 999 for t in a), default=0)
    high = max((abs(a[t][2] - b[t][2]) if t in b else 999 for t in a), default=0)
    ok = want['pages'] == got['pages'] and far <= 0.5 and high <= 0.5
    g = groups.setdefault(name.split('|')[0], [0, 0])
    g[0] += 1
    g[1] += ok
    if not ok:
        out.append(f'{name}: dx {far:.2f} dy {high:.2f}' + (f'\n  C {a}\n  L {b}' if verbose else ''))
print(' '.join(f'{g}={s}/{n}' for g, (n, s) in groups.items()))
print('total', sum(s for _, s in groups.values()), 'of', len(chrome))
print(*out, sep='\n')
