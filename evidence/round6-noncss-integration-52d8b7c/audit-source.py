"""Audit replay against the original topic and upstream CSS trees without editing either."""
from pathlib import Path
import collections,hashlib,json,subprocess,sys,tomllib
p=Path(__file__).resolve().parent;out=Path(sys.argv[1]);cache=p.parents[2]
base='52d8b7c576c82ff38b5709f989dca24a8cba141d';prior='4bf8195b4d87ad340501bb564ed2cea29b06d75f'
def git(*args):return subprocess.check_output(['git',*args])
def g(*args):return git(*args).decode().strip()
sha=lambda f:hashlib.sha256(f.read_bytes()).hexdigest()
head=(p/'commit.txt').read_text().strip().split()[0]
assert head==g('rev-parse',head) and base==g('merge-base',base,head)
ancestor=g('merge-base',prior,base);assert ancestor.startswith('2845483ec')
def changed(a,b):return set(g('diff','--name-only',a,b).splitlines())
up=changed(ancestor,base);old=changed(ancestor,prior);new=changed(base,head)
def blob(tree,path):
 r=subprocess.run(['git','rev-parse','-q','--verify',tree+':'+path],capture_output=True)
 return r.stdout.decode().strip() if not r.returncode else None
assert new==old,(new-old,old-new)
preserve=up-old
assert all(blob(head,s)==blob(base,s) for s in preserve)
assert all(blob(head,s)==blob(prior,s) for s in old-up)
both=sorted(up&old)
def ids(a,b):
 records=[]
 for h in g('rev-list','--reverse',a+'..'+b).splitlines():
  raw=git('diff-tree','--no-commit-id','--binary','-p',h+'^',h)
  fingerprint=subprocess.check_output(['git','patch-id','--stable'],input=raw).decode().split()[0]
  records.append([h,fingerprint,g('show','-s','--format=%s',h)])
 return records
left,right=ids(ancestor,prior),ids(base,head)
assert len(left)==len(right)==22 and [r[1] for r in left]==[r[1] for r in right]
assert [s[2] for s in left]==[s[2] for s in right]
# For overlapping files independently apply only the original topic diff to a temporary base blob via git apply.
import tempfile
with tempfile.TemporaryDirectory() as tmp:
 root=Path(tmp)
 subprocess.run(['git','init','-q',tmp],check=True)
 for path in both:
  f=root/path;f.parent.mkdir(parents=True,exist_ok=True);f.write_bytes(git('show',base+':'+path))
  patch=git('diff','--binary',ancestor,prior,'--',path)
  run=subprocess.run(['git','-C',tmp,'apply','--whitespace=nowarn','-'],input=patch,capture_output=True)
  if run.returncode:
   # Upstream context may have moved; the exact region may still apply with a shorter hunk context.
   f.write_bytes(git('show',base+':'+path))
   patch=git('diff','--unified=1','--binary',ancestor,prior,'--',path)
   subprocess.run(['git','-C',tmp,'apply','--whitespace=nowarn','-'],input=patch,check=True)
  assert f.read_bytes()==git('show',head+':'+path),path
assert not any(s.startswith(('bench/wpt/','crates/pyro-jsc','crates/pyro-runtime/')) or 'expectation' in s.lower() for s in new)
def packages(rev):return {(d['name'],d['version'],d.get('source'),d.get('checksum')) for d in tomllib.loads(git('show',rev+':Cargo.lock').decode())['package']}
assert packages(base)==packages(head)
jsc='0e0dd7038a7a-6f4649e395e0';lib=sha(cache/'jsc'/jsc/'lib/libJavaScriptCore.a');assert lib=='3deb71f8e7d14daf174e35340a4ff0c1900b0e9f576503adeb5facbb1201f029'
bins={}
for lab,stem in [('base','base'),('head','candidate')]:
 digest=sha(p/stem);assert digest==(p/(stem+'.sha256')).read_text().split()[0];bins[lab]=digest
s=dict(base=base,prior=prior,head=head,commonAncestor=ancestor,jsc=jsc,jscArchiveSHA256=lib,binarySHA256=bins,upstreamOnlyPreserved=sorted(preserve),topicOnlyPreserved=sorted(old-up),overlapThreeWayApplied=both,patches=[dict(prior=a[0],head=b[0],subject=a[2],priorPatchID=a[1],headPatchID=b[1]) for a,b in zip(left,right)],lockedPackageIdentities=len(packages(base)),changedPaths=sorted(new),changedTestExpectations=False,shippedWasmBackendChanged=False)
out.mkdir(parents=True,exist_ok=True);(out/'source-audit.json').write_text(json.dumps(s,indent=1)+'\n')
print(len(left),len(preserve),len(old-up),both,bins)
