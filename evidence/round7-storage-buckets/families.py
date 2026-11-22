from pathlib import Path
import collections,gzip,json
w=Path(__file__).resolve().parent;groups=json.loads((w/'groups.json').read_bytes());pages={r['url']:r for r in json.loads((w/'base.json').read_bytes())['results']};out=collections.defaultdict(collections.Counter)
def group(u):
 if u in groups['guards']:return 'neighboring StorageManager guards'
 if 'idlharness' in u:return 'bucket interfaces'
 if 'bucket_names' in u or 'buckets_basic' in u:return 'catalog names and lifecycle'
 if 'quota-indexeddb' in u:return 'IndexedDB quota'
 return 'storage policy and bucket operations'
for u,n,i,b,p,c in map(json.loads,gzip.open(w/'named.jsonl.gz','rt')):
 for k in [group(u),'environment: '+pages[u]['environment']]:
  t=out[k];t['matchedGain']+=b is not None and b!='Pass' and p=='Pass';t['newPass']+=b is None and p=='Pass';t['lost']+=b=='Pass' and p!='Pass';t['signedOnly']+=c=='Pass' and p!='Pass';t['lpOnly']+=p=='Pass' and c!='Pass'
(w/'families.json').write_text(json.dumps(out,indent=2)+'\n')
