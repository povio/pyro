from pathlib import Path
import hashlib,json,subprocess,sys,zipfile
work=Path(sys.argv[1]); shipped=work/'shipped'
def sha(p):
 with p.open('rb') as f:return hashlib.file_digest(f,'sha256').hexdigest()
def read(p):return json.loads(p.read_bytes())
head='516dc3159ff4602bd3a11e98607c262c0ffc65c4';zipsha='0da0b5ce58aa78ab4575efbdc7760e9df6dbb34e7dbf0116f0f9c470700926a7';binary='9b2454c277a52452150d59b33c64c7d46f0206781c0068f5a6b0dd0631ffd87e'
a=read(shipped/'github-artifact.json'); r=read(shipped/'github-run.json')
assert a['id']==14745758172 and r['id']==a['workflow_run']['id']==44998826328 and a['name']=='archive-arm64'
assert r['head_sha']==a['workflow_run']['head_sha']==head and r['head_branch']==a['workflow_run']['head_branch']=='v0.0.50-rc.5'
z=shipped/'archive-arm64.zip';assert z.stat().st_size==a['size_in_bytes'] and sha(z)==zipsha and a['digest']=='sha256:'+zipsha
with zipfile.ZipFile(z) as archive:
 assert archive.namelist().count('pyro')==1
 with archive.open('pyro') as stream:assert hashlib.file_digest(stream,'sha256').hexdigest()==binary
i=read(work/'machine-provenance.json');assert i['head']==head and i['tree']=='39850661325fc24f3cd9e9f591d14df55a5fa359'
exe=Path(i['bin']);assert sha(exe)==sha(shipped/'pyro')==i['binarySHA256']==binary
arch=subprocess.check_output(['file','-b',str(exe)],text=True).strip();assert arch=='Mach-O 64-bit executable arm64'
c=work.parents[1]
for p,k,x in [(c/'round6-reference-158/census/runner/bench/wpt/run-no-native-retry.mjs','runnerSHA256','b82c52329f881a5193e21ceb22634517c4cb776bd39bb080692c24eb21ac594a'),(work/'plans-v2/tasks.json','effectiveManifestSHA256','4a3badc8428aa0743c1747b1aecf70082d30e1a69cd9e6a17d8ce200eeb6a63e'),(c/'round6-inventory/MANIFEST-pinned.json','manifestSHA256','34a67512a84d43630f7adbecebd4378df5f410446591015cc6a20a3034e02819')]:
 assert i[k]==sha(p)==x
print(json.dumps(dict(tag=r['head_branch'],head=head,tree=i['tree'],workflow=r['id'],artifact=a['id'],archiveSHA256=zipsha,executableSHA256=binary,architecture=arch,runMetadataObservation=r,machine=i),indent=2))
