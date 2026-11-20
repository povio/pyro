from pathlib import Path
import json,collections,gzip,sys
d=Path(__file__).parent
for section in ['target','guard']:
 runs={k:json.load(open(d/f'{section}-{k}.json')) for k in ['aa',sys.argv[1] if len(sys.argv)>1 else 'candidate','chrome158']}
 def names(p):
  counts=collections.Counter();out={}
  for s in p['subtests']:
   n=s['name'];counts[n]+=1;out[n,counts[n]]=s['status']
  return out
 groups=collections.defaultdict(collections.Counter);raw=[];ps={}; sets={k:{p['url']:p for p in r['results']} for k,r in runs.items()}
 assert len({tuple(sorted(v)) for v in sets.values()})==1
 for u in sorted(next(iter(sets.values()))):
  cols={k:names(v[u]) for k,v in sets.items()};keys=set.union(*(set(v) for v in cols.values()));t=collections.Counter()
  for label,rows in sets.items():
   p=rows[u];v=cols[label];t[label+'_pass']=sum(s=='Pass' for s in v.values());t[label+'_full']=int(p['status']=='complete' and p.get('harnessStatus','OK')=='OK' and bool(v) and all(s=='Pass' for s in v.values()));t[label+'_page_'+p['status']]+=1
  for key in keys:
   a,b,r=[v.get(key) for v in cols.values()];raw.append([u,*key,a,b,r]);t['gained']+=a!='Pass' and b=='Pass';t['lost']+=a=='Pass' and b!='Pass';t['missing']+=a is not None and b is None;t['new']+=b is not None and a is None;t['candidateOnly']+=b=='Pass' and r!='Pass';t['chromeOnly']+=r=='Pass' and b!='Pass'
  groups['all'].update(t);ps[u]=dict(t)
 f=sys.argv[1] if len(sys.argv)>1 else 'candidate'
 (d/f'{section}-{f}-summary.json').write_text(json.dumps(dict(total=groups['all'],pages=ps,restarts={k:r['restarts'] for k,r in runs.items()}),indent=1)+'\n')
 with gzip.open(d/f'{section}-{f}-names.jsonl.gz','wt') as out:
  for r in sorted(raw):print(json.dumps(r),file=out)
 print(section,groups['all'])
