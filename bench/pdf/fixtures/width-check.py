#!/usr/bin/env python3
"""Compare a probe of generators/fx-width.mjs with Chrome 156's marker positions and link rects.

usage: width-check.py chrome-156/width-x.json probe.json
       width-check.py --extract chrome-probe.json chrome-156/width-x.json

`tbl1` is left out: an empty percentage-width table cell is wrong for another reason.
"""
import json
import sys

SKIP = {'tbl1'}


def extract(run):
    words = {w['t']: [round(w['x'], 2), round(w['x1'], 2)] for page in run['words'] for w in page if w['t'] not in SKIP}
    rects = [[round(v, 2) for v in r] for page in run['rects'] for r in page]
    return {'pages': run['pages'], 'words': words, 'rects': rects}


if sys.argv[1] == '--extract':
    probe = json.load(open(sys.argv[2]))
    json.dump({k: extract(v) for k, v in probe.items()}, open(sys.argv[3], 'w'), separators=(',', ':'))
    sys.exit()

chrome, lp = json.load(open(sys.argv[1])), json.load(open(sys.argv[2]))
groups, seen = {}, []
for name, want in chrome.items():
    got = extract(lp[name])
    g = groups.setdefault(name.split('|')[0], [0, 0])
    g[0] += 1
    words = all(t in got['words'] and abs(x[0] - got['words'][t][0]) <= 0.15 for t, x in want['words'].items())
    rects = len(want['rects']) == len(got['rects']) and all(
        abs(a - b) <= 0.15 for r, s in zip(want['rects'], got['rects']) for a, b in zip(r, s)
    )
    if want['pages'] == got['pages'] and words and rects:
        g[1] += 1
    else:
        seen.append(name)
for g, (total, same) in groups.items():
    print(f'{g:12} {same}/{total}')
print('total', sum(g[1] for g in groups.values()), 'of', len(chrome))
if seen:
    print('differ:', ' '.join(seen))
