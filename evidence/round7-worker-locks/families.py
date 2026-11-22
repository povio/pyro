from pathlib import Path
import collections,gzip,json
w=Path(__file__).resolve().parent;groups=json.loads((w/'groups.json').read_text());pages={p['url']:p for p in json.loads((w/'base.json').read_text())['results']};out=collections.defaultdict(collections.Counter)
def family(u):
 if u in groups['guards']:return 'DOM AbortSignal guards'
 if u.startswith('web-locks/bfcache/'):return 'document BFCache scenarios'
 return pages[u]['environment']
for u,n,i,b,p,c in map(json.loads,gzip.open(w/'named.jsonl.gz','rt')):
 t=out[family(u)];t['matchedGain']+=b is not None and b!='Pass' and p=='Pass';t['newPass']+=b is None and p=='Pass';t['lost']+=b=='Pass' and p!='Pass';t['signedOnly']+=c=='Pass' and p!='Pass';t['lpOnly']+=p=='Pass' and c!='Pass'
for u,b,p,c in json.loads((w/'pages.json').read_text()):
 t=out[family(u)];t['baseStrict']+=b[2];t['candidateStrict']+=p[2];t['signedStrict']+=c[2]
(w/'families.json').write_text(json.dumps(out,indent=2)+'\n')
