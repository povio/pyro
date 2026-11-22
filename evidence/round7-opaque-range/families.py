from pathlib import Path
import collections,gzip,json
w=Path(__file__).resolve().parent;groups=json.loads((w/'groups.json').read_bytes())
def group(url):
 if url in groups['guards']:return 'neighboring guards'
 if any(x in url for x in ('geometry','display-none','highlight')):return 'geometry and highlights'
 if 'interactive' in url or 'event-order' in url:return 'user edits and timing'
 return 'creation offsets and lifecycle'
out=collections.defaultdict(collections.Counter)
for u,n,i,b,p,c in map(json.loads,gzip.open(w/'named.jsonl.gz','rt')):
 t=out[group(u)];t['matchedGain']+=b is not None and b!='Pass' and p=='Pass';t['newPass']+=b is None and p=='Pass';t['lost']+=b=='Pass' and p!='Pass';t['signedOnly']+=c=='Pass' and p!='Pass';t['lpOnly']+=p=='Pass' and c!='Pass'
(w/'families.json').write_text(json.dumps(out,indent=2)+'\n')
