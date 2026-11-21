"""Check source identity, patch order, immutable trees and binary provenance."""
import hashlib,json,subprocess,sys
from pathlib import Path
p=Path(__file__).resolve().parent;out=Path(sys.argv[1]);r=p/'src'
sha=lambda f:hashlib.sha256(f.read_bytes()).hexdigest()
def git(*a):return subprocess.check_output(['git','-C',str(r),*a])
def rev(x):return git('rev-parse',x).decode().strip()
def paths(a,b):return set(git('diff','--name-only',a,b).decode().splitlines())
def commits(a,b):return git('rev-list','--reverse',a+'..'+b).decode().splitlines()
def patch(x):return subprocess.check_output(['git','patch-id','--stable'],input=git('diff',x+'^',x)).decode().split()[0]
base=rev('b62d66442');oldbase=rev('52d8b7c57');nav=rev('bab023c81');dom=rev('93117170d');head=(p/'commit.txt').read_text().strip()
orig=commits(oldbase,nav)+[dom];curr=commits(base,head);assert len(orig)==len(curr)==26
patches=[]
for a,b in zip(orig,curr,strict=True):
 x=patch(a);assert patch(b)==x
 patches.append(dict(original=a,integrated=b,stablePatchID=x))
up=paths(oldbase,base);topic=paths(oldbase,nav);inert=paths(dom+'^',dom)
assert not up & (topic|inert)
assert paths(base,head)==topic|inert
assert all(rev(head+':'+s)==rev(base+':'+s) for s in up)
for s in topic|inert:
 expected=dom if s in inert else nav
 assert rev(head+':'+s)==rev(expected+':'+s),s
assert not any('expectation' in s.lower() or s.startswith('bench/wpt/') for s in (topic|inert))
lib=p.parents[2]/'jsc/0e0dd7038a7a-6f4649e395e0/lib/libJavaScriptCore.a'
assert sha(lib)=='3deb71f8e7d14daf174e35340a4ff0c1900b0e9f576503adeb5facbb1201f029'
bins={k:sha(p/k) for k in ('base','head')}; assert all((p/(k+'.sha256')).read_text().split()[0]==v for k,v in bins.items())
build={}
for lab,sub,logname in [('base','base-src','build-base.log'),('head','src','build-head-clean.log')]:
 f=p/logname;txt=f.read_text();assert 'exit 0' in txt
 packages={}
 for line in txt.splitlines():
  if 'Compiling ' in line and str(p/sub)+'/' in line:
   name=line.split('Compiling ')[1].split()[0];packages[name]=line.split(str(p/sub)+'/')[1].rstrip(')')
 assert {'html5ever','taffy','blitz-dom','pyro-dom','pyro-webapi','pyro-runtime','pyro-browser','pyro-cdp','pyro'}<=packages.keys()
 build[lab]={'sha256':sha(f),'compiledLocalPackages':packages,'binarySHA256':bins[lab]}
out.mkdir(exist_ok=True);(out/'build.json').write_text(json.dumps(build,indent=1)+'\n');(out/'source.json').write_text(json.dumps(dict(base=base,head=head,oldBase=oldbase,stack=nav,domParser=dom,changes=patches,unchangedCSSPaths=sorted(up),topicPaths=sorted(topic|inert),binarySHA256=bins,jscArchiveSHA256=sha(lib)),indent=1)+'\n')
