from pathlib import Path
import collections,gzip,hashlib,json,tarfile,io
a=Path(__file__).resolve().parent;root=a.parent;cache=a.parents[2];old=cache/'round7-local/census-1f1be0324';p=root/'plans';s=p/'selections'
s.mkdir(parents=True,exist_ok=True)
load=lambda f:json.loads(f.read_bytes())
sha=lambda f:hashlib.sha256(f.read_bytes()).hexdigest()
def copy(src,dst):
 b=src.read_bytes()
 if dst.exists():assert dst.read_bytes()==b
 else:dst.write_bytes(b)
def save(dst,obj):
 b=(json.dumps(obj,indent=1)+'\n').encode()
 if dst.exists():assert dst.read_bytes()==b
 else:dst.write_bytes(b)
x=load(a/'scope-summary.json');tasks=load(a/'effective-manifest.json');omit=set(load(a/'omit-urls.json'))
copy(a/'effective-manifest.json',p/'tasks.json');copy(a/'omit-urls.json',p/'omit-urls.json');copy(a/'scope-summary.json',p/'scope-summary.json')
for t in tasks:
 k=t['key'];orig=old/'selections'/(k+'.json');src=a/'changed-selections'/orig.name if t['excluded'] else orig
 copy(src,s/orig.name);assert sha(s/orig.name)==t['sha256'] and load(s/orig.name)==[u for u in load(orig) if u not in omit]
for side in ['local','studio']:
 save(p/(side+'-tasks.json'),[t for t in tasks if t['owner']==side])
page=[];names=[];counts=collections.Counter()
with gzip.open(old/'evidence/pages.jsonl.gz') as f:
 for line in f:
  r=json.loads(line)
  if r[0] in omit:
   page.append(r)
   for label,col in [('priorLP',1),('signedChrome158',2)]:
    assert r[col];counts[label+'Strict']+=r[col][2];counts[label+'Passing']+=r[col][3];counts[label+'Observed']+=r[col][4]
with gzip.open(old/'evidence/named.jsonl.gz') as f:
 for line in f:
  if not line.startswith(tuple(('["'+u+'"').encode() for u in omit)):continue
  r=json.loads(line);names.append(r)
  lp,c=r[3:5];lp=lp and lp[0]=='Pass';c=c and c[0]=='Pass';counts['bothPass']+=bool(lp and c);counts['priorLPOnly']+=bool(lp and not c);counts['signedChromeOnly']+=bool(c and not lp)
assert {r[0] for r in page}==omit
save(p/'excluded-baseline-impact.json',dict(source=x['originalPublication'],observedFinalHead=False,counts=counts,pages=sorted(page),named=sorted(names)))
for t in tasks:
 t['reason']='See omit-urls.json and scope-summary.json'
(p/'README.md').write_text('''# Matched cumulative LP census selections

The ordered tasks.json entries use zero-based ordinals in the published 320-shard planner. Even ordinals belong to the local machine and odd ones to Studio. The count and sha256 fields describe the exact selection file under selections/. Original counts, checksums and excluded URLs appear beside each effective selection. The scope summary records the reasons and the checks against previous holds.

All ordinary retained URLs keep their original order within the shard. Five explicitly crash-named fixtures were withheld before execution. Their prior LP and signed Chrome observations remain separate in excluded-baseline-impact.json and are excluded from the new matched denominator. The 1,469 URLs already withheld by the prior census remain outside the plan. This yields 30,353 eligible URLs, comprising 15,150 local and 15,203 Studio. No browser inputs were executed to produce or verify this plan.
''')
(p/'SHA256SUMS').write_text(''.join(sha(f)+'  '+str(f.relative_to(p))+'\n' for f in sorted(p.rglob('*')) if f.is_file() and f.name!='SHA256SUMS'))
# Normalize archive metadata so both hosts can verify a single transport artifact.
bundle=root/'plans-c3372ad21.tar.gz'
with bundle.open('wb') as raw, gzip.GzipFile(filename='',mode='wb',fileobj=raw,mtime=0) as gz,tarfile.open(fileobj=gz,mode='w|') as tf:
 for f in sorted(p.rglob('*')):
  if f.is_file():
   b=f.read_bytes();q=tarfile.TarInfo('plans/'+str(f.relative_to(p)));q.size=len(b);q.mode=0o644;q.mtime=0;tf.addfile(q,io.BytesIO(b))
print('tasks',sha(p/'tasks.json'),'bundle',sha(bundle),'SHA256SUMS',sha(p/'SHA256SUMS'),'excluded',dict(counts))
