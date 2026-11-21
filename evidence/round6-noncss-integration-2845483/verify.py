"""Read back every exported page and named occurrence without source receipts."""
import collections,gzip,hashlib,json,sys
from pathlib import Path
p=Path(sys.argv[1]);G=lambda n:json.loads((p/n).read_bytes())
def each(n):
 with gzip.open(p/n,'rt') as f:
  for l in f:yield json.loads(l)
stats=collections.defaultdict(collections.Counter);urls={};original=G('selected-urls.json')
for u,k,co,scope,d,b,h,c in each('pages.jsonl.gz'):
 assert u not in urls and original[u]==[k,co,scope]
 for pg in [b,h,c]:
  if pg:assert pg[2]==(pg[0]=='complete' and pg[1]=='OK' and pg[4]>0 and pg[4]==pg[3])
 ct=stats[co+'/'+scope];ct['planned']+=1
 for lab,pg in zip(('base','head','chrome'),(b,h,c)):
  if not pg:ct[lab+'/missingPage']+=1
  else:ct[lab+'/page/'+pg[0]]+=1;ct[lab+'/fullPages']+=pg[2];ct[lab+'/passes']+=pg[3]
 for i,j,tag in ((b,h,'base-to-head'),(c,h,'chrome-to-head')):
  if i and j:ct[tag+'/pairedPages']+=1;ct[tag+'/gainedFullPages']+=int(j[2] and not i[2]);ct[tag+'/lostFullPages']+=int(i[2] and not j[2])
 urls[u]=(ct,[b,h,c])
assert set(urls)==set(original)
sub=collections.defaultdict(lambda:[[0,0] for _ in range(3)]);last=None
for u,n,occ,b,h,c in each('named.jsonl.gz'):
 key=(u,n,occ);assert last is None or last<key
 assert occ==(last[2]+1 if last and (u,n)==last[:2] else 1);last=key
 ct,pgs=urls[u];d=[b,h,c]
 for j,x in enumerate(d):
  if x:
   assert pgs[j] and len(x)==2;sub[u][j][0]+=int(x[0]=='Pass');sub[u][j][1]+=1
 for left,right,tag in ((0,1,'base-to-head'),(2,1,'chrome-to-head')):
  if not pgs[left] or not pgs[right]:continue
  a=d[left];b=d[right]
  av=a is not None and a[0]=='Pass';bv=b is not None and b[0]=='Pass'
  ct[tag+'/gainedPass']+=int(bv and not av);ct[tag+'/lostPass']+=int(av and not bv);ct[tag+'/commonPass']+=int(av and bv)
  ct[tag+'/addedName']+=int(a is None and b is not None);ct[tag+'/missingName']+=int(a is not None and b is None)
for u,(ct,pgs) in urls.items():
 for j,row in enumerate(pgs):
  if row:assert sub[u][j]==row[3:5]
assert stats==G('summary.json')
hist=G('historical-292.json');assert len(hist)==292 and len({(r['url'],r['canonical'],r['occurrence']) for r in hist})==292
counts=collections.defaultdict(lambda:collections.defaultdict(collections.Counter))
for r in hist:
 for lab in ['base','head','chrome']:
  val='unobserved-page' if not r[lab+'Page'] else 'missing-name' if r[lab] is None else r[lab][0]
  counts['all'][lab][val]+=1
  if r['url'].startswith('referrer-policy/'):counts['referrer'][lab][val]+=1
assert counts==G('historical-summary.json')
print(f'Verified {len(urls)} unique URLs; every strict-page and named status, reported occurrence, pass-set delta and historical total agreed.')
