import collections,gzip,json,pathlib,sys
p=pathlib.Path(sys.argv[1]);hist=json.loads((p/'historical-292.json').read_text())
key=lambda r:(r['url'],r['canonical'],r['occurrence'])
wanted={key(t) for t in hist};urls={t['url'] for t in hist}
pg={};obs={}
for line in gzip.open(p/'cbdeb-pages.jsonl.gz','rt'):
 u,h,c,k=json.loads(line)
 if u in urls:pg[u]=[h,c]
for line in gzip.open(p/'cbdeb-names.jsonl.gz','rt'):
 u,n,i,h,c=json.loads(line);k=u,n,i
 if k in wanted:obs[k]=[h,c]
counts=collections.defaultdict(lambda:collections.defaultdict(collections.Counter))
for t in hist:
 rec=pg.get(t['url']);h,c=obs.get(key(t),[None,None]);t['cbdeb']=h;t['cbdebPage']=rec[0] if rec else None
 assert c==t['chrome158'] or t['cbdebPage'] is None
 for label,x,page in [('cbdeb',h,t['cbdebPage']),('fadc',t['fadc'],True),('chrome158',t['chrome158'],t['chrome158Page'])]:
  value='unobserved-page' if not page else 'missing-name' if x is None else x[0]
  counts['all'][label][value]+=1
  if t['url'].startswith('referrer-policy/'):counts['referrer'][label][value]+=1
(p/'historical-cbdeb-292.json').write_text(json.dumps(hist,indent=1)+'\n')
(p/'historical-cbdeb-summary.json').write_text(json.dumps(counts,indent=2)+'\n')
print(json.dumps(counts))
