from pathlib import Path
import collections,gzip,json
w=Path(__file__).parent;g=json.loads((w/'groups.json').read_text())
def family(u,n):
 if u in g['guards']:return 'neighboring guards'
 if u.startswith('svg/animations/'):return 'SMIL length-list animations'
 if 'SVGLengthList' in n or 'SVGAnimatedLengthList' in n:return 'explicit list interface'
 return 'other assertions on target pages'
scores=collections.defaultdict(collections.Counter)
for u,n,i,b,p,c in map(json.loads,gzip.open(w/'named.jsonl.gz','rt')):
 x=scores[family(u,n)];x['matchedGain']+=b is not None and b!='Pass' and p=='Pass';x['newPass']+=b is None and p=='Pass';x['lost']+=b=='Pass' and p!='Pass';x['signedOnly']+=c=='Pass' and p!='Pass';x['lpOnly']+=p=='Pass' and c!='Pass'
(w/'families.json').write_text(json.dumps(scores,indent=2)+'\n')
