from pathlib import Path
import gzip,hashlib,json,shutil,subprocess
w=Path(__file__).resolve().parent
dest=w/'package'
def sha(path):
 with path.open('rb') as f:return hashlib.file_digest(f,'sha256').hexdigest()
def git(ref):return subprocess.check_output(['git','-C',str(w/'source'),'rev-parse',ref],text=True).strip()
for scope in [Path('.'),Path('video-queue')]:
 source=w/scope;target=dest/scope;target.mkdir(exist_ok=True,parents=True)
 files=['urls-unique.json','run.py','report.py','pages.json','summary.json','named.jsonl.gz','base.log','candidate.log','chrome158-map.json']
 if scope==Path('.'):
  files+=['urls.json','groups.json','selector-reconciliation.json','candidate.sh','native.sh','signed-reference.py','families.py','cohort-scores.json','signals-held.json','package.py','acquire.sh']
  for name in ['ordinary','transport','close','capability','pcm']:
   files+=[name+'.cjs']+[l+'-'+name+'.json' for l in ['base','candidate','chrome158']]
 for name in files:shutil.copy(source/name,target/name)
 for name in ['base.json','candidate.json','chrome158.json']+(['base-build.log','candidate-build.log','native.log'] if scope==Path('.') else []):
  (target/(name+'.gz')).write_bytes(gzip.compress((source/name).read_bytes(),mtime=0))
for name in ['first-candidate','second-candidate']:
 source=w/name; target=dest/'iterations'/name; target.mkdir(parents=True,exist_ok=True)
 for f in source.iterdir():
  if f.suffix in ['.json','.log']:
   (target/(f.name+'.gz')).write_bytes(gzip.compress(f.read_bytes(),mtime=0))
  else:shutil.copy(f,target/f.name)
out={'parent':git('636c1afc4'),'parentTree':git('636c1afc4^{tree}'),'candidateBuiltRevision':(w/'built-revision.txt').read_text().strip(),'topicRevision':git('HEAD'),'topicTree':git('HEAD^{tree}'),'baselineBinarySHA256':sha(w/'base-pyro'),'candidateBinarySHA256':sha(w/'pyro'),'buildProfile':'wptiter/render, -j1','jscArchiveKey':'0e0dd7038a7a-6f4649e395e0','jscArchiveSHA256':'3deb71f8e7d14daf174e35340a4ff0c1900b0e9f576503adeb5facbb1201f029','reference':json.loads((w/'chrome158-map.json').read_text())['referenceIdentity']}
(dest/'provenance.json').write_text(json.dumps(out,indent=2)+'\n')
