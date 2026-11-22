"""Package an already validated even-shard comparison on a fast-forward evidence branch."""
from pathlib import Path
import collections,fcntl,gzip,hashlib,json,shutil,subprocess,sys
d=Path(__file__).resolve().parent;w=d.parent/'census-release-516dc3159'
snap=Path(sys.argv[1]).resolve();n=int(sys.argv[2]);assert n in (1,40,80,120,160)
repo=d/'publication';prefix='evidence/rc5-census-516dc3159-even';branch='evidence/rc5-census-516dc3159-even'
out=repo/prefix;machine=out/'machine';machine.mkdir(parents=True,exist_ok=True)
def sha(path):
 with path.open('rb') as f:return hashlib.file_digest(f,'sha256').hexdigest()
def cp(src,dst):
 dst.parent.mkdir(parents=True,exist_ok=True);shutil.copyfile(src,dst)
def read(p):return json.loads(p.read_bytes())
def git(*args):return subprocess.check_output(['git','-C',str(repo),*args],text=True).strip()
meta=read(w/'machine-provenance.json');proven=read(snap/'current-provenance.json');summary=read(snap/'summary.json')
assert n==len(proven)==summary['shardsComplete'] and all(x['owner']=='local' and x['index']%2==0 for x in proven.values())
assert summary['head']=='516dc3159ff4602bd3a11e98607c262c0ffc65c4'
with (w/'raw/completed.jsonl').open() as f:
 fcntl.flock(f,fcntl.LOCK_SH);rows=[json.loads(l) for l in f if l.strip()]
rows=[r for r in rows if r['key'] in proven];assert len(rows)==n
raw=machine/'raw';raw.mkdir(exist_ok=True)
counts=collections.Counter()
for row in rows:
 key=row['key'];v=proven[key];assert v['receiptSHA256']==row['receiptSHA256'] and v['machine']['binarySHA256']==meta['binarySHA256']
 assert {k:v[k] for k in ('index','owner','count','selectionSHA256','logSHA256')}=={k:row[k] for k in ('index','owner','count','selectionSHA256','logSHA256')}
 source=w/'raw'/(key+'.json');target=raw/(key+'.json.gz');assert sha(source)==row['receiptSHA256']
 if not target.exists():
  with target.open('wb') as of,gzip.GzipFile(filename='',mode='wb',compresslevel=3,fileobj=of,mtime=0) as gz,source.open('rb') as f:shutil.copyfileobj(f,gz,1024*1024)
 with gzip.open(target,'rb') as gz:assert hashlib.file_digest(gz,'sha256').hexdigest()==row['receiptSHA256']
 for suffix in ('.log','.start.json'):cp(w/'raw'/(key+suffix),raw/(key+suffix))
 cp(w/'verified-ports'/(key+'.json'),machine/'verified-ports'/(key+'.json'))
 counts.update(row['statuses'])
(raw/'completed.jsonl').write_text(''.join(json.dumps(r,separators=(',',':'))+'\n' for r in rows))
for name in ['native-events.jsonl','failures.jsonl']:
 f=w/'raw'/name
 if f.exists():
  (raw/name).write_text(''.join(l+'\n' for l in f.read_text().splitlines() if json.loads(l)['key'] in proven))
for key in proven:
 attempt=w/'infrastructure-attempts'/key
 if attempt.is_dir():shutil.copytree(attempt,machine/'infrastructure-attempts'/key,dirs_exist_ok=True)
for name in ['allocation.json','machine-provenance.json']:cp(w/name,machine/name)
for name in ['github-artifact.json','github-run.json']:cp(w/'shipped'/name,out/name)
for name in ['export.sh','release_identity.py','release-identity.json','publish.py','directional.py']:cp(d/name,out/name)
for name in ['aggregate.py','reference_index.py','tracked.py','wpt_ports.py']:cp(d/'exporter'/name,out/'exporter'/name)
for f in snap.iterdir():
 if f.is_file():cp(f,out/'comparison'/f.name)
subprocess.run([sys.executable,str(d/'directional.py'),str(snap),str(out/'asymmetric')],check=True)
urls=summary['counts']['current']['pages'];assert urls==sum(counts.values())
if n==160:assert urls==15072 and summary['requestedScopeComplete'] and not summary['fullCensusComplete']
nms=summary['counts']['currentChrome'];v=summary['counts']
text=f"""# RC.5 macOS arm64 non-CSS census, local even shards

Local receipts cover {n} of 160 even ordinals, {urls:,} of 15,072 assigned URLs. The full V2 plan has 30,179 eligible URLs across 320 shards. It excludes 1,469 historical holds plus 179 additional variants. The local comparison reuses signed Google Chrome stable 158.0.8127.37 evidence from the pinned WPT a421f2fea5eb7f91e0db399e44b0fb0779ee1603. The runner uses four jobs per shard. Each page's environment, variant, harness status and missing names remain available in comparison/. The previous-LP column refers to 1f1be0324.

This executable came from the release workflow's archive-arm64 artifact 14745758172 for tag v0.0.50-rc.5. Source 516dc3159ff4602bd3a11e98607c262c0ffc65c4, Git tree 39850661325fc24f3cd9e9f591d14df55a5fa359. The complete ZIP and its inner executable were independently hashed against release metadata. The launched Mach-O arm64 executable SHA-256 was 9b2454c277a52452150d59b33c64c7d46f0206781c0068f5a6b0dd0631ffd87e. The release identity describes the shipped JavaScriptCore executable. The packaged archive supplies no standalone static JSC library for an independent library hash.

| Engine | Passing / observed named occurrences | Strict pages |
| --- | ---: | ---: |
"""
forrows=[('Shipped LP','current'),('Signed Chrome stable','chrome'),('Historical LP','previous')]
for label,name in forrows:
 a=v[name];text+=f"| {label} | {a['passingNames']:,} / {a['observedNames']:,} | {a['strictPages']:,} |\n"
text+=f"\nMatched asymmetric passes number {nms['firstOnly']:,} LP-only and {nms['secondOnly']:,} signed-Chrome-only. Current page lifecycle counts are {dict(sorted(counts.items()))}. Original raw receipt bytes can be recovered from machine/raw/*.json.gz and checked against completed.jsonl. Each launch and port proof is preserved. Historical and newly withheld URLs remain explicit in the coverage ledger.\n"
(out/'README.md').write_text(text)
sf=out/'SHA256SUMS';sf.write_text(''.join(sha(f)+'  '+str(f.relative_to(out))+'\n' for f in sorted(out.rglob('*')) if f.is_file() and f!=sf))
git('add','-f',prefix)
if subprocess.run(['git','-C',str(repo),'diff','--cached','--quiet']).returncode:
 git('commit','-m',f'Archive {n} verified even shards of the shipped RC.5 census')
tip=git('rev-parse','HEAD');git('push','origin',branch);assert git('ls-remote','--heads','origin',branch).split()[0]==tip
print(json.dumps({'shards':n,'urls':urls,'commit':tip,'status':counts,'current':v['current'],'chrome':v['chrome'],'signedOnly':nms['secondOnly'],'lpOnly':nms['firstOnly']}),flush=True)
