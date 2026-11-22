"""Join exact signed/current/previous results from two published release-machine archives."""
from pathlib import Path
import argparse,collections,gzip,hashlib,json,shutil,subprocess,sys
p=argparse.ArgumentParser();p.add_argument('--odd-machine',type=Path,required=True);p.add_argument('--odd-ref');p.add_argument('--odd-archive',type=Path);p.add_argument('--odd-sha256');p.add_argument('--even-ref',required=True);p.add_argument('--repo',type=Path,required=True);a=p.parse_args()
d=Path(__file__).resolve().parent;c=d.parent;w=c/'census-release-516dc3159';odd=a.odd_machine.resolve(); repo=a.repo.resolve()
def git(*args):return subprocess.check_output(['git','-C',str(repo),*args],text=True).strip()
def obj(p):return json.loads(p.read_bytes())
def sha(p):
 with p.open('rb') as f:return hashlib.file_digest(f,'sha256').hexdigest()
def assert_commit(ref):
 assert len(ref)==40 and git('rev-parse',ref+'^{commit}')==ref
assert_commit(a.even_ref)
if a.odd_archive:
 assert a.odd_sha256 and sha(a.odd_archive)==a.odd_sha256
else:assert_commit(a.odd_ref)
assert git('merge-base','--is-ancestor', '516dc3159ff4602bd3a11e98607c262c0ffc65c4',a.even_ref)==''
i=obj(odd/'machine-provenance.json');assert i.get('machine',i.get('owner'))=='studio' and i['binarySHA256']=='9b2454c277a52452150d59b33c64c7d46f0206781c0068f5a6b0dd0631ffd87e'
assert (i['head'],i['tree'])==('516dc3159ff4602bd3a11e98607c262c0ffc65c4','39850661325fc24f3cd9e9f591d14df55a5fa359')
for machine,owner,parity,num_urls in ((w,'local',0,15072),(odd,'studio',1,15107)):
 ledger=[json.loads(l) for l in (machine/'raw/completed.jsonl').read_text().splitlines() if l.strip()]
 assert len(ledger)==160 and len({r['key'] for r in ledger})==160 and sum(r['count'] for r in ledger)==num_urls
 assert all(r['owner']==owner and r['index']%2==parity for r in ledger)
out=d/'combined-320'
if not (out/'SHA256SUMS').exists():subprocess.run([str(d/'export.sh'),'all',str(out),'--require-complete','--run',str(odd)],check=True)
subprocess.run(['shasum','-a','256','-c','SHA256SUMS'],cwd=out,stdout=subprocess.DEVNULL,check=True)
s=obj(out/'summary.json');assert s['fullCensusComplete'] and s['shardsComplete']==320 and s['scheduledUrls']==30179
assert s['counts']['current']['pages']==30179
m=obj(out/'current-provenance.json')
assert {v['machine'].get('machine',v['machine'].get('owner')) for v in m.values()}=={'local','studio'}
name='rc5-census-516dc3159-combined';branch='evidence/'+name;stage=d/'combined-publication'
if not stage.exists():git('worktree','add','-qb',branch,str(stage),a.even_ref)
dest=stage/'evidence'/name;dest.mkdir(parents=True,exist_ok=True)
shutil.copytree(out,dest/'comparison',dirs_exist_ok=True)
subprocess.run([sys.executable,str(d/'directional.py'),str(out),str(dest/'asymmetric')],check=True)
subprocess.run([sys.executable,str(d/'mapping_audit.py'),str(out),str(dest/'asymmetric')],check=True)
audit=obj(dest/'asymmetric/port-mapping-audit.json')
for f in ['combined.py','import_studio.py','port_proofs.py','mapping_audit.py','export.sh','release_identity.py','release-identity.json','even-jsc-runtime.json','reference-index-readback.json','directional.py']:shutil.copyfile(d/f,dest/f)
shutil.copytree(d/'exporter',dest/'exporter',ignore=shutil.ignore_patterns('__pycache__'),dirs_exist_ok=True)
shutil.copyfile(odd/'machine-provenance.json',dest/'odd-normalized-provenance.json')
shutil.copyfile(odd/'producer-completed.jsonl',dest/'studio-producer-completed.jsonl')
transfer=d/'studio-transfer'
for n in ['verified-asset.json','rc5-studio-odd-516dc3159.tar.zst.verified.json','rc5-studio-odd-516dc3159-role-ports.tar.zst.verified.json']:
 if (transfer/n).is_file():shutil.copyfile(transfer/n,dest/n)
for n in ['machine-provenance.json','studio-sw-vers.txt','system-jsc-bundle-version.txt','jsc-runtime-version.txt','dyld-loaded-images.txt']:
 if (transfer/'raw'/n).is_file():shutil.copyfile(transfer/'raw'/n,dest/('studio-original-'+n))
if (transfer/'ports/role-port-proof/manifest.json').is_file():shutil.copyfile(transfer/'ports/role-port-proof/manifest.json',dest/'studio-port-manifest.json')

(dest/'raw-inputs.json').write_text(json.dumps({'even':{'commit':a.even_ref,'machineJSON':sha(w/'machine-provenance.json')},'odd':{'reference':a.odd_ref,'archiveSHA256':a.odd_sha256,'machineJSON':sha(odd/'machine-provenance.json')}},indent=2)+'\n')
v=s['counts'];r=v['currentChrome']
text=f"""# Shipped RC.5 non-CSS testharness census

Both macOS arm64 machines ran the exact RC.5 release workflow executable at 516dc3159ff4602bd3a11e98607c262c0ffc65c4, SHA-256 9b2454c277a52452150d59b33c64c7d46f0206781c0068f5a6b0dd0631ffd87e. Both hosts reported macOS 27.2.1 build 26B131 and the same dynamically loaded system JavaScriptCore UUID C083204E-71DA-3BC5-AFF4-524B5EAFF741, bundle 22625.2.8.11.9. The measured workflow archive reports fork:false. The separately packaged npm darwin-arm64 browser uses a patched static engine. The Studio archive supplies a per-ordinal completion ledger, original run logs, traces, configurations and machine/executable provenance; separate per-shard launch start records are available only in the even archive. The even partition covered 15,072 URLs locally and the odd partition covered 15,107 URLs on Studio. The 320 disjoint shards account for all 30,179 eligible URLs at WPT a421f2fea5eb7f91e0db399e44b0fb0779ee1603. The comparator was signed Chrome stable 158.0.8127.37; the historical LP comparator is 1f1be0324. Reference results retain their original execution provenance.

| Engine | Named passes / observations | Strict pages |
| --- | ---: | ---: |
"""
for label,key in [('Shipped LP','current'),('Signed Chrome stable','chrome'),('Historical LP','previous')]:
 v0=v[key];text+=f"| {label} | {v0['passingNames']:,} / {v0['observedNames']:,} | {v0['strictPages']:,} |\n"
text+=f"\nConfirmed LP-only passing occurrences total {audit['counts'].get('lpConfirmedOnly',0):,}, and confirmed Chrome-only occurrences total {audit['counts'].get('chromeConfirmedOnly',0):,}. Per-attempt port proof is missing for {len(audit['unresolvedConfigs'])} shards and affects {sum(audit['unresolvedRowsByShard'].values()):,} occurrence rows. The confirmed ledgers exclude these rows, which are retained with the source shard, original names and statuses in unresolved-mappings.jsonl.gz. Raw-identity asymmetries are reported separately in the complete comparison counters. The official inventory has 31,827 URLs, including 1,469 historical holds and 179 additional held variants. Coverage and per-environment dispositions, page errors, incomplete runs, missing named observations and native exits are preserved in comparison/. Raw-input commits retain individual receipt and launch provenance.\n"
(dest/'README.md').write_text(text)
sf=dest/'SHA256SUMS';sf.write_text(''.join(sha(f)+'  '+str(f.relative_to(dest))+'\n' for f in sorted(dest.rglob('*')) if f.is_file() and f!=sf))
subprocess.run(['git','-C',str(stage),'add','-f','evidence/'+name],check=True);subprocess.run(['git','-C',str(stage),'commit','-qm','Join both verified RC.5 non-CSS census partitions'],check=True)
tip=subprocess.check_output(['git','-C',str(stage),'rev-parse','HEAD'],text=True).strip();git('push','origin',branch);assert git('ls-remote','--heads','origin',branch).split()[0]==tip;print(tip)
