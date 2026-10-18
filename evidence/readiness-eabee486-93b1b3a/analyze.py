#!/usr/bin/env python3
"""Pure arithmetic over sanitized observer-free rows; never rewrites narrative."""
import gzip
import json
import math
import statistics as stats
import sys
from pathlib import Path

ORDER = 'ABBABAAB'

def distribution(values):
    assert values and all(math.isfinite(v) for v in values)
    return dict(n=len(values), median=stats.median(values), minimum=min(values), maximum=max(values), values=values)

def network_summary(events):
    def count(kind): return sum(e['kind'] == kind for e in events)
    return dict(requests=count('request'), responses=count('response'), finished=count('finished'), failed=count('failed'),
                cache_responses=sum(e['kind']=='response' and e.get('fromDiskCache',False) for e in events),
                decoded_bytes=sum(e.get('dataLength',0) for e in events if e['kind']=='data'),
                encoded_body_bytes=sum(e.get('encodedDataLength',0) or 0 for e in events if e['kind']=='finished'),
                unassociated_events=sum(e.get('ordinal') is None for e in events),
                events=len(events))

def compare(values):
    a=[values[i] for i in range(8) if ORDER[i]=='A']
    b=[values[i] for i in range(8) if ORDER[i]=='B']
    reduction=lambda x,y:100*(1-y/x) if x else None
    pairs=[]
    for i in range(0,8,2):
        ai=i if ORDER[i]=='A' else i+1
        bi=i if ORDER[i]=='B' else i+1
        pairs.append(dict(A_launch=ai+1,B_launch=bi+1,A=values[ai],B=values[bi],reduction_pct=reduction(values[ai],values[bi])))
    blocks=[]
    for block in range(2):
        av=stats.median([values[i] for i in range(4*block,4*block+4) if ORDER[i]=='A'])
        bv=stats.median([values[i] for i in range(4*block,4*block+4) if ORDER[i]=='B'])
        blocks.append(dict(block=block+1,A=av,B=bv,reduction_pct=reduction(av,bv)))
    gain=reduction(stats.median(a),stats.median(b))
    return dict(A=distribution(a),B=distribution(b),median_reduction_pct=gain,pairs=pairs,blocks=blocks,
                passes_descriptive_gate=gain is not None and gain>=5 and all(x['B']<x['A'] for x in blocks) and sum(x['B']<x['A'] for x in pairs)>=3)

def analyze(rows):
    assert len(rows)==8, 'Partial matrix: retain raw data; no complete comparison'
    navs=[]
    for launch,p in enumerate(rows,1):
        assert p.get('completed') and not p.get('failed'), f'incomplete launch{launch}'
        assert len(p['navigations'])==3
        assert p['cacheMiB']==64
        for n in p['navigations']:
            start=n['startHostMs'];usable=n['usableHostMs'];end=n['observationEndHostMs']
            assert start<usable<end
            cpu=lambda s:s['userNs']+s['systemNs']
            row=dict(launch=launch,label=ORDER[launch-1],ordinal=n['navigationOrdinal'],usable_ms=usable-start,
                     commit_ms=n['commitHostMs']-start,dcl_ms=n['dclHostMs']-start,load_ms=n['loadHostMs']-start,
                     cpu_to_usable_ms=(cpu(n['usable'])-cpu(n['before']))/1e6,
                     cpu_after_usable_ms=(cpu(n['end'])-cpu(n['usable']))/1e6,
                     rss_usable_MiB=n['usable']['rssKiB']/1024,rss_end_MiB=n['end']['rssKiB']/1024,
                     horizon_after_usable_ms=end-usable,
                     network_to_usable=network_summary([e for e in n['network'] if e['hostMs']<=usable]),
                     network_horizon=network_summary(n['network']))
            assets=[e['hostMs']-start for e in n['network'] if e['kind']=='finished' and e.get('type') in ['Script','Stylesheet','Image','Font','Media']]
            row['last_asset_within_horizon_ms']=max(assets) if assets else None
            navs.append(row)
        assert [n['navigationOrdinal'] for n in p['navigations']]==[1,2,3]
    groups={}
    for name,ordinals in [('fresh',[1]),('reused',[2,3]),('nav2',[2]),('nav3',[3])]:
        groups[name]={}
        for metric in ['usable_ms','load_ms','cpu_to_usable_ms','rss_usable_MiB']:
            values=[stats.median([n[metric] for n in navs if n['launch']==launch and n['ordinal'] in ordinals]) for launch in range(1,9)]
            groups[name][metric]=compare(values)
    return dict(order=ORDER,navigations=navs,comparisons=groups,
                setup_ms=[p['setupEndMs']-p['setupStartMs'] for p in rows],
                notes=['Primary units are four processes per binary; reused median(nav2,nav3).','Descriptive gate for usable time only; do not infer significance/causality from secondary metrics.'])

if __name__=='__main__':
    path=Path(sys.argv[1])
    data=gzip.decompress(path.read_bytes()) if path.suffix=='.gz' else path.read_bytes()
    print(json.dumps(analyze(json.loads(data)),indent=2))
