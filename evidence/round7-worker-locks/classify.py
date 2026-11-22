from pathlib import Path
from collections import Counter,defaultdict
import json,gzip
w=Path(__file__).resolve().parent
groups=json.loads((w/'groups.json').read_text())
rows=[json.loads(line) for line in gzip.open(w/'named.jsonl.gz','rt')]
cat=lambda u:'guards' if u in groups['guards'] else 'target'
counts=defaultdict(Counter)
for u,n,i,b,p,c in rows:
 k=counts[cat(u)]
 for l,v in [('base',b),('candidate',p),('chrome',c)]:
  k[l+'Named']+=v=='Pass';k[l+'Observed']+=v is not None
 k['matchedGain']+=b not in (None,'Pass') and p=='Pass'
 k['newlyObservedPass']+=b is None and p=='Pass'
 k['lostPass']+=b=='Pass' and p!='Pass'
 k['candidateOnlyPass']+=p=='Pass' and c!='Pass'
 k['chromeOnlyPass']+=c=='Pass' and p!='Pass'
 k['recoveredChromeOnly']+=c=='Pass' and b!='Pass' and p=='Pass'
 k['missingFromCandidate']+=b is not None and p is None
 k['newlyObserved']+=b is None and p is not None
for u,*v in json.loads((w/'pages.json').read_text()):
 for l,p in zip(['base','candidate','chrome'],v):counts[cat(u)][l+'Strict']+=p[2]
(w/'cohort-scores.json').write_text(json.dumps(counts,indent=2)+'\n')
print(json.dumps(counts,indent=2))
