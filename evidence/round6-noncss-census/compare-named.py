"""Full-outer comparison by pinned URL, exact test name, and duplicate occurrence.
Usage: python3 compare-named.py inventory.json reference.jsonl.gz local.json out.jsonl.gz [--prefix /dom/]
Reference accepts the archived wpt.fyi normalized JSONL or a run.mjs JSON.
"""
import argparse
from collections import Counter
import gzip
import json
from pathlib import Path

def load(path):
    return json.loads(Path(path).read_text())

def normalized(url):
    return '/' + url.lstrip('/')

def run_records(doc):
    seen = set()
    for page in doc['results']:
        url = normalized(page['url'])
        if url in seen:
            raise ValueError(f'{url}: repeat or duplicate URL; select a single measurement per side')
        seen.add(url)
        yield url, page

def pack(page, is_run):
    if page is None:
        return 'missing', None, {}, None
    status = page['status']
    if is_run:
        harness = page.get('harnessStatus')
        state = 'complete' if status == 'complete' and harness in (None, 'OK') else 'partial'
        rows = ((t['name'], t['status']) for t in page['subtests'])
    else:
        harness = status
        state = 'complete' if status == 'OK' else ('skipped' if status == 'SKIP' else 'partial')
        rows = page['subtests']
    occurrences = Counter()
    names = {}
    for name, result in rows:
        occurrences[name] += 1
        names[name, occurrences[name]] = result.upper().replace(' ', '')
    return state, status, names, harness

def compare(inventory, reference, local_doc, dest, prefix):
    records = {normalized(row['url']): row for group in ('eligible', 'inventoryOnly')
               for row in inventory[group] if normalized(row['url']).startswith(prefix)}
    local = dict(run_records(local_doc))
    stats = Counter()
    seen = set()
    def one(out, url, ref, run_ref):
        if url not in records:
            return
        if url in seen:
            raise ValueError(f'duplicate reference URL {url}')
        seen.add(url)
        rs, rh, a, r_harness = pack(ref, run_ref)
        ls, lh, b, l_harness = pack(local.get(url), True)
        stats['reference_'+rs+'_pages'] += 1
        stats['local_'+ls+'_pages'] += 1
        stats['pages_'+rs+'_vs_'+ls] += 1
        entry = records[url]
        names = []
        full = lambda state, d: None if state=='missing' else state=='complete' and bool(d) and all(v=='PASS' for v in d.values())
        rf, lf = full(rs, a), full(ls, b)
        stats['reference_fully_passing_pages'] += rf is True
        stats['local_fully_passing_pages'] += lf is True
        for key in sorted(a.keys() | b.keys()):
            left, right = a.get(key), b.get(key)
            names.append([*key, left, right])
            stats['reference_named_pass'] += left=='PASS'
            stats['local_named_pass'] += right=='PASS'
            if left=='PASS' and right=='PASS':
                stats['shared_named_pass'] += 1
            for side, value, other, other_state in [('reference',left,right,ls),('local',right,left,rs)]:
                if value=='PASS' and other!='PASS':
                    stats[side+'_only_named_pass'] += 1
                    reason = ('missing_page' if other_state=='missing' else
                              'unreported_on_'+other_state+'_page' if other is None else 'observed_nonpass')
                    stats[side+'_only_pass_opposite_'+reason] += 1
        out.write(json.dumps(dict(url=url,environment=entry.get('environment'),inventoryReason=entry.get('reason'),
            referenceState=rs,localState=ls,referenceStatus=rh,localStatus=lh,
            referenceHarness=r_harness,localHarness=l_harness,referenceFullyPassing=rf,
            localFullyPassing=lf,names=names),separators=(',',':'))+'\n')
    with gzip.open(dest,'wt') as out:
        if str(reference).endswith('.gz'):
            with gzip.open(reference,'rt') as stream:
                for line in stream:
                    row=json.loads(line)
                    one(out,normalized(row['url']),row,False)
        else:
            for url,row in run_records(load(reference)):
                one(out,url,row,True)
        for url in sorted(records.keys()-seen):
            one(out,url,None,False)
    stats['corpus_pages']=len(records)
    stats['local_extra_pages']=len({u for u in local if u.startswith(prefix)}-records.keys())
    return stats

if __name__=='__main__':
    p=argparse.ArgumentParser()
    for arg in ['inventory','reference','local','output']:p.add_argument(arg)
    p.add_argument('--prefix',default='/')
    a=p.parse_args()
    print(json.dumps(compare(load(a.inventory),a.reference,load(a.local),a.output,a.prefix),indent=2,sort_keys=True))
