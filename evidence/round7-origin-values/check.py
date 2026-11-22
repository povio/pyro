from pathlib import Path
import json
w=Path(__file__).parent
def rows(l,suffix='probe'):return json.loads((w/f'{l}-{suffix}.json').read_text())
s,c=rows('chrome158'),rows('candidate')
diff={k:dict(signed=s.get(k),candidate=c.get(k)) for k in sorted(c.keys()|s.keys()) if s.get(k)!=c.get(k)}
sb,cb=rows('chrome158','blob-links'),rows('candidate','blob-links')
bd={k:dict(signed=sb.get(k),candidate=cb.get(k)) for k in sorted(cb.keys()|sb.keys()) if sb.get(k)!=cb.get(k)}
(w/'direct-differences.json').write_text(json.dumps({'basic':diff,'supplemental':bd},indent=2)+'\n')
print('basic differences',list(diff),'supplemental differences',list(bd))
