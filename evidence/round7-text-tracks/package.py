from pathlib import Path
import gzip,hashlib,json,shutil,subprocess
w=Path(__file__).resolve().parent;p=w/'package';p.mkdir(exist_ok=True)
def sha(f):
 with f.open('rb') as a:return hashlib.file_digest(a,'sha256').hexdigest()
def rev(s):return subprocess.check_output(['git','-C',str(w/'source'),'rev-parse',s],text=True).strip()
for name in ['urls.json','groups.json','pages.json','named.jsonl.gz','summary.json','cohort-scores.json','base.log','candidate.log','base-provenance.txt','chrome158-map.json','signed-reference.py','report.py','cohorts.py','run.py','acquire.sh','candidate.sh','native.sh','package.py','captions.cjs','replacement.cjs','region.cjs','realms.cjs','playback.cjs','reattach.cjs','equivalent.cjs','held.json']:
 shutil.copy(w/name,p/name)
for n in ['captions','replacement','region','realms','playback','reattach','equivalent']:
 for l in ['base','candidate','chrome158']:shutil.copy(w/(l+'-'+n+'.json'),p/(l+'-'+n+'.json'))
for name in ['base.json','candidate.json','chrome158.json','base-build.log','candidate-build.log','native.log']:
 (p/(name+'.gz')).write_bytes(gzip.compress((w/name).read_bytes(),mtime=0))
for folder in sorted(w.glob('iteration-*')):
 for f in folder.rglob('*'):
  if not f.is_file():continue
  dest=p/folder.name/f.relative_to(folder);dest.parent.mkdir(exist_ok=True,parents=True)
  if f.suffix in ['.json','.log']:dest.with_name(dest.name+'.gz').write_bytes(gzip.compress(f.read_bytes(),mtime=0))
  else:shutil.copy(f,dest)
prov=dict(parent=rev('59c4ba28f'),parentTree=rev('59c4ba28f^{tree}'),candidateBuiltRevision=(w/'built-revision.txt').read_text().strip(),topicRevision=rev('HEAD'),topicTree=rev('HEAD^{tree}'),baselineBinaryDetail='Editing shared exact parent, rehashed',baselineBinarySHA256=sha(w/'base-pyro'),candidateBinarySHA256=sha(w/'pyro'),frozenSelectorSHA256=sha(w/'urls.json'),buildProfile='wptiter/render -j1',jscArchiveKey='0e0dd7038a7a-6f4649e395e0',jscArchiveSHA256='3deb71f8e7d14daf174e35340a4ff0c1900b0e9f576503adeb5facbb1201f029',standardsSHA256={n:sha(w/n) for n in ['media-spec.html','webvtt-spec.html']},reference=json.loads((w/'chrome158-map.json').read_text())['referenceIdentity']);(p/'provenance.json').write_text(json.dumps(prov,indent=2)+'\n')
