"""Independently reconstruct directional pass sets, page totals and missing-name counts."""
import collections,gzip,json
from pathlib import Path
import sys
def read(f):
 with gzip.open(f,'rt') as s:yield from map(json.loads,s)
def verify(p,complete):
 st=json.loads((p/'summary.json').read_bytes());page={};ct=collections.defaultdict(collections.Counter);district=collections.defaultdict(collections.Counter);status=collections.Counter()
 for u,h,c,b,ids,why in read(p/'pages.jsonl.gz'):
  assert u not in page;page[u]=([h,c,b],[[0,0] for _ in range(3)])
  assert all(bool(i)==bool(s) for i,s in zip(ids,[h,c,b]))
  if not h:continue
  assert c and b and not why;status[h[0]]+=1
  for k,ref in [('headChrome',c),('headPrevious',b)]:
   ct[k].update(matched=1,firstFull=int(h[2]),secondFull=int(ref[2]),firstOnlyFull=int(h[2] and not ref[2]),secondOnlyFull=int(ref[2] and not h[2]))
 assert len(page)==31827 and sum(v[0][1] is not None for v in page.values())==30359
 hist={(x['url'],x['canonical'],x['occurrence']):x for x in json.loads((p/'historical-292.json').read_bytes())};last=None;verified_hist=set()
 for u,n,i,h,c,b in read(p/'named.jsonl.gz'):
  assert (i==last[2]+1 if last and last[:2]==(u,n) else i==1);last=u,n,i
  pgs,tracked=page[u]
  if last in hist:assert hist[last]['round7']==h;verified_hist.add(last)
  for j,x in enumerate([h,c,b]):
   if x:
    assert len(x)==2 and pgs[j];tracked[j][0]+=x[0]=='Pass';tracked[j][1]+=1
  if not pgs[0]:continue
  for k,y in [('headChrome',c),('headPrevious',b)]:
   if h is None and y is None:continue
   xpass=h is not None and h[0]=='Pass';ypass=y is not None and y[0]=='Pass'
   d=ct[k];d['firstPass']+=xpass;d['secondPass']+=ypass;d['commonPass']+=xpass and ypass
   d['firstOnly']+=xpass and not ypass;d['secondOnly']+=ypass and not xpass
   d['missingFirstName']+=h is None;d['missingSecondName']+=y is None
   area=district[u.split('/')[0]];area[k+'FirstOnly']+=xpass and not ypass;area[k+'SecondOnly']+=ypass and not xpass
 for u,(pgs,sizes) in page.items():
  for pg,size in zip(pgs,sizes):
   if pg:assert pg[3:5]==size and pg[2]==bool(pg[0]=='complete' and pg[1]=='OK' and size[1] and size[0]==size[1])
 for k,r in hist.items():
  assert r['round7Page']==page[k[0]][0][0]
  if k not in verified_hist:assert r['round7'] is None
 assert len(hist)==292 and ct==st['comparisons'] and status==st['lifecycle']
 if complete:
  assert st['completeShards']==st['totalShards']==320 and st['missingEligible']==0 and st['matched']==30358
  assert ct['headChrome']['secondPass']==1755476 and ct['headPrevious']['secondPass']==1688041
 (p/'areas.json').write_text(json.dumps(district,indent=1)+'\n')
 print('Readback verified',st['matched'],'/',st['scheduled'],flush=True)
if __name__=='__main__': verify(Path(sys.argv[1]),'--partial' not in sys.argv)
