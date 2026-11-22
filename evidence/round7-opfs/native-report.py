from pathlib import Path
w=Path(__file__).parent
lines=[]
for name in ('native-store.log','native-stream.log','native-worker.log','native-descriptors.log','native-manager.log','native-manager-final.log'):
    p=w/name
    if p.exists():
        lines.append(name)
        lines.extend(x for x in p.read_text().splitlines() if x.startswith(('running ','test ','assertion ','  left:',' right:','error: test failed')))
(w/'native-results.txt').write_text('\n'.join(lines)+'\n')
