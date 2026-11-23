#!/usr/bin/env python3
"""Check that a stripped Mach-O copy has the same code and data as its original.

usage: same-code.py ORIGINAL STRIPPED

strip(1) and a new ad-hoc signature rewrite only the load commands and
__LINKEDIT (symbol table, signature). Every section's contents must stay
byte-for-byte the same, so a copy that cannot be run here (the Intel browser on
Apple silicon without Rosetta) behaves like the original.
"""
import subprocess
import sys


def sections(path):
    """{segment,section: (file offset, size)} for sections with file contents."""
    out = subprocess.run(['otool', '-l', path], check=True, capture_output=True, text=True).stdout
    found, cur = {}, {}
    for line in out.splitlines():
        words = line.split()
        if words[:1] == ['Section']:
            cur = {}
        elif len(words) == 2 and words[0] in ('sectname', 'segname', 'size', 'offset'):
            cur[words[0]] = words[1]
            if words[0] == 'offset' and 'sectname' in cur:
                found[cur['segname'] + ',' + cur['sectname']] = (int(cur['offset']), int(cur['size'], 16))
                cur = {}
    return found


original, stripped = sys.argv[1:]
a, b = sections(original), sections(stripped)
if set(a) != set(b):
    sys.exit(f'sections differ: {sorted(set(a) ^ set(b))}')
da, db = open(original, 'rb').read(), open(stripped, 'rb').read()
total = 0
for name in sorted(a):
    (oa, size), (ob, size_b) = a[name], b[name]
    if size != size_b:
        sys.exit(f'{name}: {size} bytes in {original}, {size_b} in {stripped}')
    if oa == 0:  # zero-fill (__bss, __common): no file contents
        continue
    if da[oa:oa + size] != db[ob:ob + size]:
        sys.exit(f'{name} differs between {original} and {stripped}')
    total += size
print(f'{len(a)} sections, {total} bytes identical')
