"""Check all archived observations, their profile membership and their census overlap."""
from pathlib import Path
import collections,gzip,json,sys
root=Path(sys.argv[1]);read=lambda n:json.loads((root/n).read_bytes())
def lines(n):
 with gzip.open(root/n,'rt') as s:yield from map(json.loads,s)
profiles=read('archive-provenance.json');stats=read('archive-comparisons.json');smoke=set();frozen=set();page={};counts=collections.defaultdict(collections.Counter);olds=collections.Counter();news=collections.Counter()
for k,u,old,new in lines('archive-pages.jsonl.gz'):
 assert (k,u) not in page;page[k,u]=(old,new);f=profiles[k]
 if f['head'].startswith('1f1be0324'):assert f['profile']['jobs']==1; smoke.add(u)
 else:assert f['head'].startswith('305840f45') and not u.startswith('css/');frozen.add(u)
 if new:
  counts[k].update(matched=1,firstFull=int(new[2]),secondFull=int(old[2]),firstOnlyFull=int(new[2] and not old[2]),secondOnlyFull=int(old[2] and not new[2]))
for k,u,n,i,old,new in lines('archive-named.jsonl.gz'):
 pg,match=page[k,u]
 if old:olds[k,u,'reported']+=1;olds[k,u,'pass']+=old[0]=='Pass'
 if new:assert match;news[k,u,'reported']+=1;news[k,u,'pass']+=new[0]=='Pass'
 if match:
  a=new is not None and new[0]=='Pass';b=old is not None and old[0]=='Pass'
  counts[k].update(firstPass=int(a),secondPass=int(b),commonPass=int(a and b),firstOnly=int(a and not b),secondOnly=int(b and not a),missingFirstName=int(new is None),missingSecondName=int(old is None))
for (k,u),(o,n) in page.items():
 assert o[3:5]==[olds[k,u,'pass'],olds[k,u,'reported']]
 if n:assert n[3:5]==[news[k,u,'pass'],news[k,u,'reported']]
assert len(smoke)==48 and len(frozen)==stats['distinctSourceURLs']['frozen'] and counts==stats['byReceipt']
print('Verified archived observations',len(page),'from',len(profiles),'receipts')
