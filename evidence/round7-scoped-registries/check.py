"""Compare structured ordinary entry points with original signed Chrome observations."""
import json
from pathlib import Path
p=Path(__file__).parent
b,c,ref=[dict(json.loads((p/f'{l}-probe.json').read_text())) for l in ('base','candidate','chrome158')]
assert c.keys()==ref.keys()
diff={k:{'candidate':c[k],'signed':r} for k,r in ref.items() if c[k]!=r}
(p/'direct-differences.json').write_text(json.dumps(diff,indent=2)+'\n')
print(f'{len(c)-len(diff)}/{len(c)} match signed Chrome; different keys: '+', '.join(diff))
