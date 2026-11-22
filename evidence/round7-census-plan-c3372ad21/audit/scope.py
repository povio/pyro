from pathlib import Path
import collections, gzip, hashlib, json, os, re, subprocess
a=Path(__file__).resolve().parent
cache=a.parents[2]
old=cache/'round7-local/census-1f1be0324'
e=old/'evidence'
sha=lambda b: hashlib.sha256(b).hexdigest()
load=lambda f: json.loads(f.read_bytes())
def save(f,x):
    f.write_text(json.dumps(x,indent=1,sort_keys=False)+'\n')
basepub='01f49f43da24a20ecb20d99c967fc5de54272120'
pub='evidence/round7-noncss-census-1f1be0324/'
def published(f):
    return subprocess.check_output(['git','show',basepub+':'+pub+f])
tasks=load(old/'tasks.json')
assert (old/'tasks.json').read_bytes()==published('tasks.json')
for f in ['holds.json','observed-native-events.json','original-official-inventory.jsonl.gz','frozen-chrome-provenance.json']:
    assert (e/f).read_bytes()==published(f)
inventory={r['url']:r for r in map(json.loads,gzip.open(e/'original-official-inventory.jsonl.gz'))}
original=[r for r in inventory.values() if r['scope']=='non-CSS']
retained={};duplicates=[];names={}
for i,t in enumerate(tasks):
    f=old/'selections'/(t['key']+'.json');us=load(f)
    assert sha(f.read_bytes())==t['sha256'] and len(us)==len(set(us))==t['count']
    for u in us:
        if u in retained:duplicates.append(u)
        r=inventory[u]; assert r['scope']=='non-CSS' and r['eligible'] and not r['withheld']
        retained[u]=[i,t['key']]
        if re.search(r'(?:crash|vulnerab|(?:^|[/_.-])cve[-_]?\d|use[-_]?after[-_]?free|exploit|double[-_]?free|heap[-_]overflow|segfault)',u,re.I):
            names[u]=['explicitly named crash/reproduction test']
assert len(retained)==30358 and len(tasks)==320 and not duplicates
holds={}
files=[(e/'holds.json','prior census holds'),(e/'observed-native-events.json','prior census native events'),(cache/'round6-reference-158/census/export-final/studio-native-markers.json','Studio markers'),(cache/'round6-reference-158/census/plans/native-observed.json','known native holds'),(cache/'round6-single-head-evidence/evidence/round6-single-head-158/supplement/additional-holds.json','supplement holds'),(cache/'round6-noncss-measure/manifest-runs/census-8e7144b7e/new-holds.json','previous sweep'),(cache/'round6-noncss-measure/deferred-classification/union-hold.json','differential holds')]
for f,kind in files:
    x=load(f);ks=set(x['urls'] if isinstance(x,dict) and 'urls' in x else x)
    holds[str(f.relative_to(cache))]={'sha256':sha(f.read_bytes()),'records':len(ks),'overlap':len(ks & retained.keys())}
    for u in ks & retained.keys():names.setdefault(u,[]).append(kind)
# Check newer local receipts without executing their inputs.
cut=(old/'plan.py').stat().st_mtime-60
sources={};issues=[];skipped={'target','src','.git','node_modules','vendor','build','runner','worktree','repo','cargo-target','base-src','head-src','crates','packages','wpt','site-packages','source','build-source','selections','verified-ports','verified','ports','guard-ports','profiles','jsc','obj','candidate-target','base-target','.venv','qa-ports'}
roots=[cache/'round6-noncss-measure',*sorted(cache.glob('round7-*')),cache/'css-values-a7f8',cache/'site-deploy-results']
for root in roots:
    for d,dirs,fs in os.walk(root):
        dirs[:]=[n for n in dirs if n not in skipped and 'target' not in n and (Path(d)/n)!=a.parent and not (Path(d)/n/'.git').exists()]
        for n in fs:
            if not n.endswith(('.json','.json.partial')):continue
            f=Path(d)/n
            try:
                if f.stat().st_mtime<cut:continue
                with f.open('rb') as h:prefix=h.read(1500)
                if not re.search(rb'"kind"\s*:\s*"testharness"',prefix):continue
                x=load(f)
            except (OSError,ValueError) as ex:
                issues.append([str(f.relative_to(cache)),type(ex).__name__]);continue
            if x.get('wptRevision')!='a421f2fea5eb7f91e0db399e44b0fb0779ee1603':continue
            rows=x['results'];bad=[r for r in rows if r.get('status') in ('hang','crash','panic')]
            amb=bool(x.get('watchdogKills')) or (x.get('restarts') or 0)>len(bad)
            hits=[(r['url'],r['status']) for r in (rows if amb else bad)]
            if amb and len(rows)<len(x.get('plan',[])):
                hits=[(u,'unattributed process exit') for u in x['plan']]
            short=str(f.relative_to(cache));sources[short]=[len(rows),len(bad),x.get('restarts'),x.get('watchdogKills')]
            for u,status in hits:
                if u in retained:names.setdefault(u,[]).append(short+': '+status)
omissions=sorted(names)
save(a/'omit-urls.json',omissions)
safe=a/'changed-selections';safe.mkdir(exist_ok=True)
manifest=[]
for i,t in enumerate(tasks):
    key=t['key'];f=old/'selections'/(key+'.json');us=load(f);kept=[u for u in us if u not in names]
    if kept!=us:f=safe/(key+'.json');save(f,kept)
    manifest.append(dict(index=i,key=key,owner='local' if i%2==0 else 'studio',originalCount=len(us),count=len(kept),originalSHA256=t['sha256'],sha256=sha(f.read_bytes()),excluded=[u for u in us if u in names]))
save(a/'effective-manifest.json',manifest)
save(a/'later-receipts.json',dict(cutoff=cut,receipts=sources,unreadable=issues))
save(a/'scope-summary.json',dict(originalPublication=basepub,source='c3372ad21df84bb4bb7386ae4f114fa185d10f30',taskOrderSHA256=sha((old/'tasks.json').read_bytes()),originalOfficialNonCss=len(original),originalEligible=sum(r['eligible'] for r in original),frozenUrls=len(retained),duplicateUrls=len(duplicates),originallyWithheld=len(original)-len(retained),holdsChecked=holds,laterReceipts=len(sources),unreadable=issues,omissions=names,omitSHA256=sha((a/'omit-urls.json').read_bytes()),effectiveManifestSHA256=sha((a/'effective-manifest.json').read_bytes()),ownership={k:dict(shards=sum(z['owner']==k for z in manifest),original=sum(z['originalCount'] for z in manifest if z['owner']==k),effective=sum(z['count'] for z in manifest if z['owner']==k)) for k in ['local','studio']}))
print((a/'scope-summary.json').read_text())
