from pathlib import Path
import json,gzip,hashlib,subprocess,shutil
w=Path(__file__).resolve().parent;p=w/'package';p.mkdir(exist_ok=True)
def sha(f):
 with f.open('rb') as a:return hashlib.file_digest(a,'sha256').hexdigest()
def rev(s):return subprocess.check_output(['git','-C',str(w/'source'),'rev-parse',s],text=True).strip()
for n in ['base.log','candidate.log','groups.json','urls.json','pages.json','summary.json','named.jsonl.gz','base-provenance.txt','chrome158-map.json','signed-reference.py','run.py','report.py','candidate.sh','acquire.sh','native.sh','report-package.py']:
 shutil.copy(w/n,p/n)
for n in ['commands','shadow','boundaries','reflection']:
 for f in [w/(n+'.cjs'),*[w/(l+'-'+n+'.json') for l in ('base','candidate','chrome158')]]:shutil.copy(f,p/f.name)
for n in ['base.json','candidate.json','chrome158.json','base-build.log','candidate-build.log','native.log']:(p/(n+'.gz')).write_bytes(gzip.compress((w/n).read_bytes(),mtime=0))
for folder in [w/'initial']:
 if folder.exists():
  for f in folder.rglob('*'):
   if not f.is_file():continue
   q=p/folder.name/f.relative_to(folder);q.parent.mkdir(exist_ok=True,parents=True)
   if f.suffix in ['.json','.log']:q.with_name(q.name+'.gz').write_bytes(gzip.compress(f.read_bytes(),mtime=0))
   else:shutil.copy(f,q)
(p/'provenance.json').write_text(json.dumps(dict(parent=rev('59c4ba28f'),parentTree=rev('59c4ba28f^{tree}'),built=(w/'built-revision.txt').read_text().strip(),topic=rev('HEAD'),topicTree=rev('HEAD^{tree}'),parentBinary=sha(w/'base-pyro'),candidateBinary=sha(w/'pyro'),selector=sha(w/'urls.json'),reference=json.load(open('chrome158-map.json'))['referenceIdentity'],jscArchiveKey='0e0dd7038a7a-6f4649e395e0',jscArchiveSHA256='3deb71f8e7d14daf174e35340a4ff0c1900b0e9f576503adeb5facbb1201f029',standards={n:sha(w/n) for n in ['forms-spec.html','interaction-spec.html']}),indent=2)+'\n')
