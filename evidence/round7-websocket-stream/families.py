from pathlib import Path
import collections,gzip,json
w=Path(__file__).resolve().parent;groups=json.loads((w/'groups.json').read_bytes());pages={r['url']:r for r in json.loads((w/'base.json').read_bytes())['results']};out=collections.defaultdict(collections.Counter)
def groups_for(u):
 if u in groups['guards']:return ['classic guards']
 key=u.split('/')[-1].split('.any')[0];variant=u.split('?',1)[1] if '?' in u else 'plain'
 return [key,'environment: '+pages[u]['environment'],'variant: '+variant]
for u,n,i,b,p,c in map(json.loads,gzip.open(w/'named.jsonl.gz','rt')):
 for k in groups_for(u):
  t=out[k];t['matchedGain']+=b is not None and b!='Pass' and p=='Pass';t['newPass']+=b is None and p=='Pass';t['lost']+=b=='Pass' and p!='Pass';t['signedOnly']+=c=='Pass' and p!='Pass';t['lpOnly']+=p=='Pass' and c!='Pass'
(w/'families.json').write_text(json.dumps(out,indent=2)+'\n')
