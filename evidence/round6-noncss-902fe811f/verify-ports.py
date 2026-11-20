#!/usr/bin/env python3
"""Independent sampled SHA-to-raw-config provenance check for distributed port maps; stdout only."""
import argparse,collections,functools,gzip,hashlib,json,pathlib,re,sqlite3
P=pathlib.Path;a=argparse.ArgumentParser();a.add_argument('export',type=P);a.add_argument('--census',type=P,default=P.home()/'Library/Caches/lightningpanda/round6-noncss-measure/manifest-runs/census-902fe811f');a.add_argument('--sample',type=int,default=96);o=a.parse_args()
c=o.census.resolve();m=c.parent;load=lambda f:json.loads(f.read_bytes());sha=lambda b:hashlib.sha256(b).hexdigest()
db=sqlite3.connect((m/'scores.sqlite').as_uri()+'?mode=ro&immutable=1',uri=True)
with gzip.open(o.export/'port-configs.json.gz','rt') as f:configs=json.load(f)
with gzip.open(o.export/'port-provenance.jsonl.gz','rt') as f:allrows=[json.loads(v) for v in f]
mapping={r[0]:r[1:] for r in allrows};assert len(mapping)==len(allrows)
sample=set(sorted(mapping,key=lambda u:sha(u.encode()))[:o.sample])
for k in ('ai-000','html-069','event-timing-000','infrastructure-000'):
 d=load(m/'chromeWpt'/(k+'.json'))
 for r in d.get('inputPartProvenance',[]):
  present=set(r['selectedUrls'])&mapping.keys()
  if present:sample.add(min(present))
shards={u:k for k in load(m/'tasks.json') for u in load(m/(k+'.json')) if u in sample}
@functools.lru_cache(8)
def receipt(f):return load(f)
def owning(f,u):
 q=receipt(f)
 if 'inputPartProvenance' in q:
  part,=[r for r in q['inputPartProvenance'] if u in r['selectedUrls']]; src=P(part['file']);assert sha(src.read_bytes())==part['sha256']
  assert next(r for r in receipt(src)['results'] if r['url']==u)==next(r for r in q['results'] if r['url']==u)
  return P(part.get('log',src.with_suffix('.log')))
 if 'inputParts' in q:
  src,=[P(n) for n in q['inputParts'] if any(r['url']==u for r in receipt(P(n))['results'])]
  assert next(r for r in receipt(src)['results'] if r['url']==u)==next(r for r in q['results'] if r['url']==u)
  return src.parent/(f.parent.name+'_'+f.stem+'.resume.log') if src.name.endswith('.finish.json') else f.with_name(f.stem+'.1.log')
 logs=list(f.parent.glob(f.stem+'.*.log'))+list(f.parent.glob(f.stem+'.log'))
 if len(logs)==1:return logs[0]
 attempts={r['attempt'] for ln in (f.parent/'completed.jsonl').read_text().splitlines() if (r:=json.loads(ln)).get('key')==f.stem and isinstance(r.get('attempt'),int) and r.get('rc')==0}
 attempt,=attempts;return f.with_name(f.stem+'.'+str(attempt)+'.log')
@functools.lru_cache(None)
def extract(log):
 entry,=re.findall(r'^wptserve on http://web-platform\.test:(\d+) \(log ([^\n]+/serve\.log)\)$',log.read_text(),re.M)
 port,name=entry;data=P(name).parent.joinpath('config.json').read_bytes();q=json.loads(data)
 assert q['ports']['http'][0]==int(port)
 return sha(data),{k:q[a] for k,a in [('ports','ports'),('browserHost','browser_host'),('serverHost','server_host')]}
count=0;visited=set()
for u in sorted(sample):
 files=[c/(shards[u]+'.json')]+[P(db.execute('select path from result where label=? and url=?',(l,u)).fetchone()[0]) for l in ('combined2','chromeWpt')]
 for idx,f in enumerate(files):
  log=owning(f,u);h,x=extract(log)
  assert mapping[u][idx]==h and configs[h]==x,(u,idx,log)
  count+=1;visited.add(h)
print(json.dumps({'exportedPages':len(mapping),'uniqueConfigs':len(configs),'sampledUrls':len(sample),'checkedOriginalReceipts':count,'uniqueOriginalConfigsInSample':len(visited)},indent=2))
