"""Verify the release archive and the npm package independently when published."""
import base64, hashlib, json, os, subprocess, tarfile
from pathlib import Path
from urllib.request import urlopen
from urllib.error import HTTPError
p=Path(__file__).resolve().parent
tag='v0.0.50-rc.5'
repo='povio/pyro'
name='pyro-rust-'+tag+'-macos-arm64'
expected='9b2454c277a52452150d59b33c64c7d46f0206781c0068f5a6b0dd0631ffd87e'
out={}
def checksum(f):
    with f.open('rb') as h: return hashlib.file_digest(h,'sha256').hexdigest()
def member(f,path):
    with tarfile.open(f,'r:gz') as z, z.extractfile(path) as h:
        return hashlib.file_digest(h,'sha256').hexdigest()
r=subprocess.run(['gh','release','view',tag,'--repo',repo,'--json','assets'],capture_output=True,text=True)
if r.returncode:
    if 'release not found' not in r.stderr:raise RuntimeError(r.stderr)
    out['githubRelease']={'status':'unpublished'}
else:
    a=next(v for v in json.loads(r.stdout)['assets'] if v['name']==name+'.tar.gz')
    f=p/a['name']
    if not f.exists():subprocess.run(['gh','release','download',tag,'--repo',repo,'--pattern',a['name'],'--dir',str(p)],check=True)
    digest=checksum(f); assert 'sha256:'+digest==a['digest']
    inner=member(f,name+'/pyro')
    out['githubRelease']=dict(status='published',assetSHA256=digest,executableSHA256=inner,matchesCensus=inner==expected)
res=subprocess.run(['npm','view','@povio/pyro@0.0.50-rc.5','name','version','dist','--json'],capture_output=True,text=True)
if res.returncode:
    if 'E404' not in res.stderr+res.stdout:raise RuntimeError(res.stderr)
    out['npm']={'status':'unpublished'}
else:
    pkg=json.loads(res.stdout)
    assert pkg['name']=='@povio/pyro' and pkg['version']=='0.0.50-rc.5'
    f=p/'npm-published-0.0.50-rc.5.tgz'
    if not f.exists():
        subprocess.run(['curl','--fail','--location','--silent','--show-error','-o',str(f.with_suffix('.partial')),pkg['dist']['tarball']],check=True)
        os.replace(f.with_suffix('.partial'),f)
    with f.open('rb') as stream:sri='sha512-'+base64.b64encode(hashlib.file_digest(stream,'sha512').digest()).decode()
    assert sri==pkg['dist']['integrity']
    inner=member(f,'package/native/darwin-arm64/pyro')
    out['npm']=dict(status='published',tarball=pkg['dist']['tarball'],integrity=sri,archiveSHA256=checksum(f),executableSHA256=inner,matchesCensus=inner==expected)
out['censusExecutableSHA256']=expected
q=p/'publication-comparison.json';t=q.with_suffix('.tmp');t.write_text(json.dumps(out,indent=2)+'\n');os.replace(t,q);print(out)
