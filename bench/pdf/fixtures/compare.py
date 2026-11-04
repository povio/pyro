#!/usr/bin/env python3
"""Compare Page.printToPDF runs of a fixture set, per group (the part of a case's name before the first `|`).

usage: compare.py A.json B.json [-v]

Each file is what probe.mjs writes, or a summary of it (`--summarize`). Prints, per group, the cases whose words are on
the same pages (assign), those with the same page count (pages), and of the assigned ones those whose lines sit at a
constant offset (ysame), with the largest spread of that offset (maxdy, pt).

usage: compare.py --summarize IN.json OUT.json

A summary keeps, for each page: word count, a hash of its sorted words, first and last sorted word, distinct y (pt).
"""
import collections
import hashlib
import json
import sys


def order(t):
    return (t[0], len(t), t) if t else ('', 0, t)


def summary(page):
    ts = sorted((w['t'] for w in page), key=order)
    ys = sorted({round(w['y'], 2) for w in page})
    return [len(ts), hashlib.sha1(' '.join(ts).encode()).hexdigest()[:10], ts[0] if ts else '', ts[-1] if ts else '', ys]


def load(path):
    data = json.load(open(path))
    return {k: [summary(p) for p in r['words']] if isinstance(r, dict) else r for k, r in data.items()}


if sys.argv[1] == '--summarize':
    json.dump(load(sys.argv[2]), open(sys.argv[3], 'w'), separators=(',', ':'))
    sys.exit()

a, b = load(sys.argv[1]), load(sys.argv[2])
verbose = '-v' in sys.argv
groups = collections.defaultdict(lambda: [0, 0, 0, 0, 0.0])
different = []
for name, x in a.items():
    if name not in b:
        continue
    y = b[name]
    g = groups[name.split('|')[0]]
    g[0] += 1
    g[2] += len(x) == len(y)
    if [p[1] for p in x] != [p[1] for p in y]:
        different.append(name)
        continue
    g[1] += 1
    # Distinct positions per page in step; pages differ when a line's rounding merges two.
    ds = [round(s - t, 2) for p, q in zip(x, y) if len(p[4]) == len(q[4]) for s, t in zip(p[4], q[4])]
    spread = max(ds) - min(ds) if ds else 0.0
    g[4] = max(g[4], spread)
    g[3] += spread <= 0.02 and all(len(p[4]) == len(q[4]) for p, q in zip(x, y))
print('%-8s %6s %6s %6s %6s %8s' % ('group', 'total', 'assign', 'pages', 'ysame', 'maxdy'))
for name, g in sorted(groups.items()):
    print('%-8s %6d %6d %6d %6d %8.3f' % (name, g[0], g[1], g[2], g[3], g[4]))
print('total', [sum(g[i] for g in groups.values()) for i in (0, 1, 2)])
if verbose:
    for name in different:
        show = lambda r: [(p[2], p[3], p[0]) for p in r]
        print('DIFF', name, 'A', show(a[name]), 'B', show(b[name]))
