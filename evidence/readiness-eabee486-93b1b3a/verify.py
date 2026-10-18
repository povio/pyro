#!/usr/bin/env python3
"""Verify retained evidence and pure arithmetic without launching a workload."""
import hashlib
import importlib.util
import json
from pathlib import Path
import subprocess
import sys

here=Path(__file__).resolve().parent
raw=here.parent/'final-readiness-eabee486-93b1b3a'
sha=lambda p:hashlib.sha256(p.read_bytes()).hexdigest()
for rel,expected in json.loads((raw/'SHA256.json').read_text()).items():
    assert sha(raw/rel)==expected, rel
assert sha(raw/'observations.json')=='c89f32fae9841ac46d34d8366adc86a874fb1974091bb07b80fdc8f7b5db3627'
old=here.parent/'profile-navigation-eabee486/observer-free-confirmation/profile-nav.e2e.ts'
assert sha(old)=='15df21569518e8f091c45a9f146f406b4369d4112b9aa553cc03ee6db2a8f564'
new=raw/'profile-nav.e2e.ts'
assert sha(new)=='dac431328d13ff35ba630ded618c6540e19c6e79dc33d43e55070529f55e0f4e'
marker='   for(let navigationOrdinal=1;navigationOrdinal<=3;navigationOrdinal++)'
a,b=old.read_text(),new.read_text()
assert a[a.index(marker):]==b[b.index(marker):]
assert b[b.index(marker):].count('await expect(')==7
actual=subprocess.check_output([sys.executable,str(here/'analyze.py'),str(raw/'observations.json')])
assert json.loads(actual)==json.loads((here/'analysis.json').read_text())
table=subprocess.check_output([sys.executable,str(here/'tables.py'),str(here/'analysis.json')])
assert table==(here/'TABLES.generated.md').read_bytes()
# Arithmetic controls independent of observed outcome.
spec=importlib.util.spec_from_file_location('readiness_analysis',here/'analyze.py')
module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module)
assert module.compare([100,90,90,100,90,100,100,90])['passes_descriptive_gate']
assert not module.compare([100,99,101,100,99,100,100,101])['passes_descriptive_gate']
n=module.network_summary([{'kind':'data','ordinal':1,'dataLength':10,'encodedDataLength':10},{'kind':'finished','ordinal':1,'encodedDataLength':8}])
assert n['decoded_bytes']==10 and n['encoded_body_bytes']==8
print('PASS:27 raw hashes, fixed collector/body/assertions,24-row analysis, tables and arithmetic controls; no workload run.')
