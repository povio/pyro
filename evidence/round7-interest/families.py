from pathlib import Path
from collections import Counter,defaultdict
import json,gzip
w=Path(__file__).parent
groups=json.loads((w/'groups.json').read_text())
def family(url):
 name=url.rsplit('/',1)[-1]
 if url in groups['guards']:return 'legacy controls'
 if 'property-reflection' in name:return 'reference target reflection matrices'
 if url=='shadow-dom/reference-target/tentative/interestfor.tentative.html':return 'reference target hover activation'
 if name=='interestelement-interface.tentative.html':return 'reflection interface'
 if name.startswith(('interestevent-interface','interestevent-dispatch-shadow')):return 'event interface and dispatch'
 if name.startswith('idlharness.'):return 'IDL harness'
 return 'other interest behavior'
scores=defaultdict(Counter)
for u,n,i,b,p,c in map(json.loads,gzip.open(w/'named.jsonl.gz','rt')):
 x=scores[family(u)];x['matchedGain']+=b is not None and b!='Pass' and p=='Pass';x['newPass']+=b is None and p=='Pass';x['lost']+=b=='Pass' and p!='Pass';x['signedOnly']+=c=='Pass' and p!='Pass';x['lpOnly']+=c!='Pass' and p=='Pass'
(w/'families.json').write_text(json.dumps(scores,indent=2)+'\n')
