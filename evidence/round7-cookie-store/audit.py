from pathlib import Path
import collections,gzip,json
w=Path(__file__).parent
def rows(f):return map(json.loads,gzip.open(f))
def group(url):
    name=url.split('/')[-1]
    if not url.startswith('cookiestore/'):return 'legacy'
    if name.startswith('idlharness'):return 'IDL'
    if 'subscribe' in name.lower() or 'subscription' in name.lower() or 'serviceworker_' in name:return 'service-worker/subscription'
    if 'event' in name:return 'window event'
    return 'query/mutation'
def diff(a,b,key=''):
    if isinstance(a,dict) and isinstance(b,dict):
        return [d for k in sorted(set(a)|set(b)) for d in diff(a.get(k),b.get(k),key+'.'+k)]
    return [] if a==b else [[key.lstrip('.'),a,b]]
seen=collections.defaultdict(collections.Counter); changes=[]
for u,n,i,b,h,c in rows(w/'named.jsonl.gz'):
    d=seen[group(u)]
    for lbl,v in zip(('base','candidate','chrome158'),(b,h,c)):
        d[lbl+'Pass']+=v=='Pass';d[lbl+'Observed']+=v is not None
    if h=='Pass' and b!='Pass':
        d['gain']+=1;d['newPass' if b is None else 'matchedGain']+=1;d['gainChrome'+str(c)]+=1
    if b=='Pass' and h!='Pass':d['loss']+=1;changes.append([u,n,i,b,h,c])
status=json.load(open(w/'scope-scores.json'));out=dict(sources=seen,lostOccurrences=changes,setup={e:{g:v['nonOK'] for g,v in groups.items()} for e,groups in status.items()})
(w/'behavior-scores.json').write_text(json.dumps(out,indent=2)+'\n')
for p in ['probe','partitioned-probe']:
    name=w/('candidate-'+p+'.json')
    if name.is_file():
        (w/(p+'-differences.json')).write_text(json.dumps(diff(json.load(open(name)),json.load(open(w/('chrome158-'+p+'.json')))),indent=2)+'\n')
