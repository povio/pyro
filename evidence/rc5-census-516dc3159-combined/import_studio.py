"""Project the RC.5 Studio producer's root completion ledger without creating launch records."""
from pathlib import Path
from collections import Counter
import hashlib,json,os,shutil,sys,tarfile
def sha(p):
 with p.open('rb') as f:return hashlib.file_digest(f,'sha256').hexdigest()
def read(p):return json.loads(p.read_bytes())
def save(p,v):p.write_text(json.dumps(v,indent=2,sort_keys=True)+'\n')
src,ports,plans,out,runnerpack=map(Path,sys.argv[1:]);meta=read(src/'machine-provenance.json')
binary='9b2454c277a52452150d59b33c64c7d46f0206781c0068f5a6b0dd0631ffd87e';head='516dc3159ff4602bd3a11e98607c262c0ffc65c4'
assert (meta['executableSHA256'],meta['tagCommit'],meta['workflowRun'],meta['artifactID'],meta['architecture'],meta['shards'],meta['urls'])==(binary,head,44998826328,14745758172,'arm64',160,15107)
assert meta['archiveSHA256']=='0da0b5ce58aa78ab4575efbdc7760e9df6dbb34e7dbf0116f0f9c470700926a7'
assert 'fork: false' in meta['runtimeVersion'] and meta['jscBundle']=='22625.2.8.11.9' and 'C083204E-71DA-3BC5-AFF4-524B5EAFF741' in str(meta['jscLoaded'])
assert '27.2.1' in meta['os'] and '26B131' in meta['os']
assert meta['wpt']=='a421f2fea5eb7f91e0db399e44b0fb0779ee1603' and meta['bun']=='1.4.4'
plan=read(plans/'tasks.json');planhash=sha(plans/'tasks.json');assert planhash=='4a3badc8428aa0743c1747b1aecf70082d30e1a69cd9e6a17d8ce200eeb6a63e'
with tarfile.open(runnerpack) as tf:runner=hashlib.file_digest(tf.extractfile('runner/bench/wpt/run-no-native-retry.mjs'),'sha256').hexdigest()
assert runner=='b82c52329f881a5193e21ceb22634517c4cb776bd39bb080692c24eb21ac594a'
rows=[json.loads(l) for l in (src/'completed.jsonl').read_text().splitlines() if l.strip()];assert [r['index'] for r in rows]==list(range(1,320,2))
out.mkdir(exist_ok=False);raw=out/'raw';raw.mkdir();(out/'verified-ports').mkdir();bins=set();ledger=[]
for r in rows:
 t=plan[r['index']];k=t['key'];p=src/k;by=read(p/'result.json')
 assert r['key']==k and t['owner']=='studio' and r['selection']==t['sha256']==sha(plans/'selections'/(k+'.json'))
 assert r['rawSHA256']==sha(p/'result.json')
 cfg=read(ports/(k+'.json'));assert cfg['logSHA256']==sha(p/'run.log')
 assert len(by['results'])==t['count'] and set(by['plan'])=={x['url'] for x in by['results']}==set(read(plans/'selections'/(k+'.json')))
 assert by['bin']=='/private/tmp/studio-rc5-odd.ySmxWp/pyro';bins.add(by['bin'])
 for source,dest in [(p/'result.json',raw/(k+'.json')),(p/'run.log',raw/(k+'.log')),(ports/(k+'.json'),out/'verified-ports'/(k+'.json'))]:os.link(source,dest)
 ledger.append(dict(key=k,owner='studio',index=t['index'],count=t['count'],receiptSHA256=r['rawSHA256'],selectionSHA256=t['sha256'],logSHA256=cfg['logSHA256'],configSHA256=(cfg['proof'] or {}).get('sha256'),statuses=Counter(x['status'] for x in by['results']),wallMs=by['wallMs'],restarts=by['restarts'],watchdogKills=by['watchdogKills'],sourceTraceSHA256=sha(p/'trace.jsonl')))
assert len(bins)==1
(raw/'completed.jsonl').write_text(''.join(json.dumps(r,separators=(',',':'))+'\n' for r in ledger))
shutil.copyfile(src/'completed.jsonl',out/'producer-completed.jsonl')
save(out/'machine-provenance.json',dict(head=head,tree='39850661325fc24f3cd9e9f591d14df55a5fa359',owner='studio',machine='studio',host=meta['host'],bin=bins.pop(),binarySHA256=binary,runnerSHA256=runner,wpt=meta['wpt'],manifestSHA256='34a67512a84d43630f7adbecebd4378df5f410446591015cc6a20a3034e02819',effectiveManifestSHA256=planhash,producerLedgerSHA256=sha(src/'completed.jsonl'),producerMachineSHA256=sha(src/'machine-provenance.json'),runnerTransferSHA256=sha(runnerpack),nativeStartRecord='absent in Studio archive; original completion ledger and release executable identity verified separately'))
