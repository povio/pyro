#!/usr/bin/env python3
"""Render only generated tables; preserve report narrative and qualifications."""
import json, sys
from pathlib import Path
s=json.loads(Path(sys.argv[1]).read_text())
print('# Generated measurement tables\n')
print('Inputs: analysis.json. Primary independent units are processes; reused is median(nav2,nav3).\n')
print('|Group / metric|Baseline median [range]|Current median [range]|Median reduction|Descriptive gate*|')
print('|---|---:|---:|---:|---|')
for group in ['fresh','reused','nav2','nav3']:
 for metric in ['usable_ms','cpu_to_usable_ms','load_ms','rss_usable_MiB']:
  r=s['comparisons'][group][metric]
  fmt=lambda v:f"{v['median']:.3f} [{v['minimum']:.3f}, {v['maximum']:.3f}]"
  gate=('pass' if r['passes_descriptive_gate'] else 'fail') if metric=='usable_ms' and group in ['fresh','reused'] else '—'
  print(f"|{group} / {metric}|{fmt(r['A'])}|{fmt(r['B'])}|{r['median_reduction_pct']:+.2f}%|{gate}|")
print('\n*Predeclared usability gate: >=5% reduction, both block medians favor current, and >=3/4 adjacent pairs favor current. This is not a significance test.\n')
print('|Launch|Build|Nav|Usable ms|Load ms|CPU to usable ms|RSS MiB|Requests to usable / horizon|Decoded bytes to usable / horizon|Encoded body bytes to usable / horizon|Cache responses horizon|Unknown events|')
print('|---:|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|')
for n in s['navigations']:
 u=n['network_to_usable'];h=n['network_horizon']
 print(f"|{n['launch']}|{n['label']}|{n['ordinal']}|{n['usable_ms']:.3f}|{n['load_ms']:.3f}|{n['cpu_to_usable_ms']:.3f}|{n['rss_usable_MiB']:.2f}|{u['requests']} / {h['requests']}|{u['decoded_bytes']} / {h['decoded_bytes']}|{u['encoded_body_bytes']} / {h['encoded_body_bytes']}|{h['cache_responses']}|{h['unassociated_events']}|")
print('\nA=eabee486; B=93b1b3a. Encoded body bytes exclude HTTP headers; decoded bytes are summed once. Data windows do not imply all application work has completed.\n')
print('|Group|Adjacent pair A/B launch|Baseline usable ms|Current usable ms|Reduction|')
print('|---|---|---:|---:|---:|')
for group in ['fresh','reused']:
 for p in s['comparisons'][group]['usable_ms']['pairs']:
  print(f"|{group}|{p['A_launch']} / {p['B_launch']}|{p['A']:.3f}|{p['B']:.3f}|{p['reduction_pct']:+.2f}%|")
