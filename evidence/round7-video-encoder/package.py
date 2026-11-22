from pathlib import Path
import gzip,hashlib,json,shutil,subprocess
w=Path(__file__).resolve().parent;p=w/'package';p.mkdir(exist_ok=True)
def sha(f):
 with f.open('rb') as a:return hashlib.file_digest(a,'sha256').hexdigest()
def rev(s):return subprocess.check_output(['git','-C',str(w/'source'),'rev-parse',s],text=True).strip()
for name in ['urls.json','groups.json','pages.json','named.jsonl.gz','summary.json','cohort-scores.json','base.log','candidate.log','chrome158-map.json','signed-reference.py','report.py','families.py','run.py','acquire.sh','candidate.sh','native.sh','package.py','pattern.cjs','decode-pattern.cjs','profiles.cjs','reconfigure.cjs','color.cjs','smpte170m-red.h264','smpte170m-stream.json','smpte170m-header.txt','make-color-source.sh','held.json']:
 shutil.copy(w/name,p/name)
for f in [*w.glob('*-pattern.json'),*w.glob('*-decoded.json'),*w.glob('*-profiles.json'),*w.glob('*-color.json'),*w.glob('*-reconfigure.json')]:
 (p/(f.name+'.gz')).write_bytes(gzip.compress(f.read_bytes(),mtime=0))
for name in ['base.json','candidate.json','chrome158.json','base-build.log','candidate-build.log','native.log']:
 (p/(name+'.gz')).write_bytes(gzip.compress((w/name).read_bytes(),mtime=0))
for iteration in ['initial','intermediate']:
 folder=w/iteration
 if not folder.exists():continue
 for f in folder.rglob('*'):
  if not f.is_file():continue
  dest=p/iteration/f.relative_to(folder);dest.parent.mkdir(exist_ok=True,parents=True)
  if f.suffix in ['.json','.log']:dest.with_name(dest.name+'.gz').write_bytes(gzip.compress(f.read_bytes(),mtime=0))
  else:shutil.copy(f,dest)
prov=dict(parent=rev('a1ede0170'),parentTree=rev('a1ede0170^{tree}'),candidateBuiltRevision=(w/'built-revision.txt').read_text().strip(),topicRevision=rev('HEAD'),topicTree=rev('HEAD^{tree}'),baselineBinarySHA256=sha(w/'base-pyro'),candidateBinarySHA256=sha(w/'pyro'),specificationSHA256=sha(w/'webcodecs-spec.html'),frozenSelectorSHA256=sha(w/'urls.json'),buildProfile='wptiter/render -j1',jscArchiveKey='0e0dd7038a7a-6f4649e395e0',jscArchiveSHA256='3deb71f8e7d14daf174e35340a4ff0c1900b0e9f576503adeb5facbb1201f029',reference=json.loads((w/'chrome158-map.json').read_text())['referenceIdentity'])
(p/'provenance.json').write_text(json.dumps(prov,indent=2)+'\n')
