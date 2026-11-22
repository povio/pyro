from pathlib import Path
import gzip,json,collections
w=Path(__file__).parent
names=list(map(json.loads,gzip.open(w/'named.jsonl.gz')))
def match(page,func):return [r for r in names if r[0].endswith(page) and func(r[1])]
tests={
  'SVG defined inert':match('pseudo-class-defined.html',lambda n:n.startswith('Without browsing context:') and 'www.w3.org/2000/svg' in n and 'a-a' in n),
  'prefix timing':match('/Document-createElementNS-prefix-timing.html',lambda n:True),
}
for k,rows in tests.items():
    print(k,[(r[1],r[3:]) for r in rows])
out={}
for group in ['scoped','legacy']:
    rows=[r for r in names if ('/registries/' in r[0])==(group=='scoped')]
    new=collections.Counter((r[3],r[4],r[5]) for r in rows if r[3]!=r[4])
    out[group]=[[*k,v] for k,v in new.items()]
(w/'named-transitions.json').write_text(json.dumps(out,indent=2)+'\n')
(w/'focused-assertions.json').write_text(json.dumps(tests,indent=2)+'\n')
