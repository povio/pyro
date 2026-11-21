from pathlib import Path
import sys,subprocess,hashlib,json,shutil,tomllib
out=Path(sys.argv[1]).resolve();n=Path(__file__).parent.resolve()
cache=n.parents[2];ref=cache/'round6-reference-158/census'
base='cbdeb11931f74c3de4417a25fb9103f1df84d54e';head='b57c35dd45d54d6781aafcb2c8242c9ad9b26e04'
git=lambda *a:subprocess.check_output(['git',*a])
sha=lambda f:hashlib.sha256(f.read_bytes()).hexdigest()
def lock(rev):
 d=tomllib.loads(git('show',rev+':Cargo.lock').decode())
 return {(p['name'],p['version'],p.get('source'),p.get('checksum')) for p in d['package']}
def deps(rev):
 return {p['name']:p.get('dependencies',[]) for p in tomllib.loads(git('show',rev+':Cargo.lock').decode())['package'] if p['name'] in ('pyro-media','pyro-webapi')}
assert lock(head)==lock(base)
paths=git('diff','--name-only',base,head).decode().splitlines()
assert not any(p.startswith(('bench/wpt/','vendor','crates/pyro-jsc','crates/pyro-runtime')) or 'expectation' in p.lower() for p in paths)
assert git('merge-base',base,head).decode().strip()==base
before=deps(base);after=deps(head)
sources=dict(base=base,head=head,baseBinarySHA256=sha(n.parent/'batch/visible'),headBinarySHA256=sha(n/'candidate'),ciJSC='0e0dd7038a7a-6f4649e395e0',ciJSCStaticArchiveSHA256='3deb71f8e7d14daf174e35340a4ff0c1900b0e9f576503adeb5facbb1201f029',changedPaths=paths,identicalLockedPackageIdentities=len(lock(head)),dependencyEntriesAdded={p:sorted(set(after[p])-set(before[p])) for p in before},experimentalWasmiIncluded=False,originalWPTOrExpectationsChanged=False)
assert sha(cache/'jsc'/sources['ciJSC']/'lib/libJavaScriptCore.a')==sources['ciJSCStaticArchiveSHA256']
meta=json.loads((cache/'round6-noncss-jsc/metadata.json').read_bytes())
assert sha(cache/'round6-noncss-jsc/patched-jsc.zip')==meta['digest'].split(':')[1]
sources['ciJSCArtifact']=dict(id=meta['id'],zipSHA256=meta['digest'],workflowHead=meta['workflow_run']['head_sha'])
assert sources['baseBinarySHA256']=='81d12af223643b8d5c1903fd528a36a5f2ff3040855f998d7bc5e546805b93b2'
assert sources['headBinarySHA256']=='296b3d4f9775c92d98d3bd7aa898efc68b1a18e90e591af5a428ad8decd4e9bf'
(out/'source-audit.json').write_text(json.dumps(sources,indent=1)+'\n')
for name in ('export.py','verify.py','effects.py','run.py','prepare.py'):shutil.copy2(n/name,out/name)
for name in ('studio-compatibility.json','cross-machine-summary.json'):shutil.copy2(ref/'export-priority'/name,out/name)
shutil.copy2(ref/'priority-publication.json',out/'chrome-reference-checkpoint.json')
shutil.copy2(ref/'plans/summary.json',out/'frozen-census-plan.json')
shutil.copy2(n.parents[1]/'manifest-runs/census-902fe811f/wpt_ports.py',out/'wpt_ports.py')
