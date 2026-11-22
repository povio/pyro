from pathlib import Path
from collections import Counter,defaultdict
import gzip,json,sys
w=Path(sys.argv[1]);changes=defaultdict(Counter);page=defaultdict(Counter)
for line in gzip.open(w/'named.jsonl.gz'):
    u,n,i,h,c,b=json.loads(line)
    flags=[r is not None and r[0]=='Pass' for r in (h,c,b)]
    area=u.split('/')[0]
    for key,condition in [('gainsVsPrior',flags[0] and not flags[2]),('lossesVsPrior',flags[2] and not flags[0]),('signedPassAmongLosses',flags[2] and not flags[0] and flags[1]),('signedOnly',flags[1] and not flags[0]),('lpOnly',flags[0] and not flags[1])]:
        if condition:
            changes[key][area]+=1
            page[key][u]+=1
obj={k:{'total':sum(v.values()),'areas':v.most_common(),'largestPages':page[k].most_common(12)} for k,v in changes.items()}
Path(sys.argv[2]).write_text(json.dumps(obj,ensure_ascii=False,indent=1)+'\n')
