from pathlib import Path
import gzip,hashlib,json,shutil,subprocess
w=Path(__file__).resolve().parent;p=w/'package';p.mkdir(exist_ok=True)
def sha(f):
 with f.open('rb') as a:return hashlib.file_digest(a,'sha256').hexdigest()
def rev(s):return subprocess.check_output(['git','-C',str(w/'source'),'rev-parse',s],text=True).strip()
for name in ['urls.json','groups.json','pages.json','named.jsonl.gz','summary.json','cohort-scores.json','base.log','candidate.log','chrome158-map.json','signed-reference.py','families.py','report.py','package.py','run.py','native.sh','acquire.sh','candidate.sh','held.json']:
 shutil.copy(w/name,p/name)
for probe in ['pixels','lifecycle','metadata','orientation']:
 for name in [probe+'.cjs',*[x+'-'+probe+'.json' for x in ['base','candidate','chrome158']]]:shutil.copy(w/name,p/name)
for name in ['base.json','candidate.json','chrome158.json','native.log','base-build.log','candidate-build.log']:
 (p/(name+'.gz')).write_bytes(gzip.compress((w/name).read_bytes(),mtime=0))
ip=p/'initial';ip.mkdir(exist_ok=True)
for f in (w/'initial').iterdir():
 if f.suffix in ['.json','.log']:(ip/(f.name+'.gz')).write_bytes(gzip.compress(f.read_bytes(),mtime=0))
 else:shutil.copy(f,ip/f.name)
prov=dict(parent=rev('b868a2047'),parentTree=rev('b868a2047^{tree}'),candidateBuiltRevision=(w/'built-revision.txt').read_text().strip(),topicRevision=rev('HEAD'),topicTree=rev('HEAD^{tree}'),baselineBinarySHA256=sha(w/'base-pyro'),candidateBinarySHA256=sha(w/'pyro'),specificationSHA256=sha(w/'webcodecs-spec.html'),frozenSelectorSHA256=sha(w/'urls.json'),buildProfile='wptiter/render -j1',jscArchiveKey='0e0dd7038a7a-6f4649e395e0',jscArchiveSHA256='3deb71f8e7d14daf174e35340a4ff0c1900b0e9f576503adeb5facbb1201f029',reference=json.loads((w/'chrome158-map.json').read_text())['referenceIdentity'])
(p/'provenance.json').write_text(json.dumps(prov,indent=2)+'\n')
