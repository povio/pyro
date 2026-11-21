from common import *
import collections,os
ident=load(ref/'reference-identity.json');eligible=set(load(ref/'plans/shared-noncss.urls.json'));assert len(eligible)==30359
assert (qa/'source').read_text().splitlines()[0]==head and sha(binary)==digest
assert sha(cache/'jsc/0e0dd7038a7a-6f4649e395e0/lib/libJavaScriptCore.a')=='3deb71f8e7d14daf174e35340a4ff0c1900b0e9f576503adeb5facbb1201f029'
assert sha(ref/'runner/bench/wpt/run-no-native-retry.mjs')==ident['noNativeReplayRunnerSHA256'] and sha(cache/'round6-inventory/MANIFEST-pinned.json')==ident['manifestSha256']
holds=collections.defaultdict(list,load(old/'new-holds.json'))
roots=[d,*cache.glob('round7-*'),cache/'css-values-a7f8',cache/'site-deploy-results']
sources={};frozen={};partial=[]
# Existing 8e audit covers earlier receipts; additionally scan every subsequent receipt, including the complete 8e and integration raw ledgers.
cut=(old/'plan.py').stat().st_mtime-900
skip={'target','src','.git','node_modules','vendor','build','runner','worktree','repo','cargo-target','base-src','head-src','crates','packages','wpt','site-packages','target-head','runner-src','selections','verified-ports','verified','ports','guard-ports','profiles'}
for root in roots:
 for path,dirs,files in os.walk(root):
  dirs[:]=[n for n in dirs if n not in skip and not n.startswith('target') and not (Path(path)/n/'.git').exists() and Path(path)/n!=p]
  for n in files:
   if not n.endswith(('.json','.json.partial')):continue
   f=Path(path)/n
   if f.stat().st_mtime<cut:continue
   with f.open('rb') as s:prefix=s.read(750)
   if b'"kind":"testharness"' not in prefix:continue
   obj=load(f)
   if obj.get('wptRevision')!=ident['wpt']:continue
   rows=obj['results'];bad=[r for r in rows if r['status'] in ('hang','crash','panic')]
   ambiguous=obj.get('watchdogKills') or obj.get('restarts',0)>len(bad)
   hits=rows if ambiguous else bad
   if ambiguous and len(rows)<len(obj.get('plan',[])):
    hits=[{'url':u,'status':'unattributed exit'} for u in obj['plan']]
   for r in hits:
    if r['url'] in eligible:holds[r['url']].append([str(f.relative_to(cache)),r['status']])
   nm=str(f.relative_to(cache));sources[nm]=[len(rows),len(bad),obj.get('restarts',0),obj.get('watchdogKills',0)]
   if n.endswith('.partial'):partial.append(nm)
   if obj.get('bin')==str(d/'combined-305840f45/pyro') and not n.endswith('.partial'):frozen[nm]=[len(rows),obj['jobs'],obj['server'],obj.get('http2',False),sha(f)]
for u in load(ref/'export-final/studio-native-markers.json')['urls']:
 if u in eligible:holds[u].append(['studio','native markers'])
assert not set(load(ref/'plans/native-observed.json'))&eligible
for group,om in load(d/'combined-305840f45/omissions.json').items():
 for u,reason in om.items():
  if u in eligible:holds[u].append(['305840f45 omissions',reason])
smoke={}
for group in ['css','guards','edit']:
 f=qa/('head-'+group+'.json');x=load(f)
 assert x['jobs']==1 and x['bin']==str(binary) and x['server']=='wptserve' and x['http2']
 for r in x['results']:smoke[r['url']]={'receipt':f.name,'group':group,'overlap':r['url'] in eligible,'held':r['url'] in holds,'freshFourJob':r['url'] in eligible-set(holds),'sourceSHA256':sha(f)}
assert len(smoke)==48 and sum(r['overlap'] for r in smoke.values())==41
tasks=[]
for t in load(ref/'plans/tasks.json'):
 key=t['key'];f=ref/'selections'/(key+'.json');assert sha(f)==t['sha256']
 us=[u for u in load(f) if u in eligible and u not in holds]
 if us:
  dest=p/'selections'/f.name;save(dest,us);tasks.append(dict(key=key,count=len(us),sha256=sha(dest),sourceSelectionSHA256=t['sha256']))
assert sum(r['count'] for r in tasks)==len(eligible-set(holds))
save(p/'tasks.json',tasks);save(p/'holds.json',holds);save(p/'smoke-overlaps.json',smoke);save(p/'recent-audit.json',dict(receipts=sources,partial=partial,latestCut=cut));save(p/'frozen-archives.json',frozen)
print(dict(eligible=len(eligible),held=len(holds),count=sum(t['count'] for t in tasks),shards=len(tasks),recentReceipts=len(sources),frozenArchives=len(frozen),partial=len(partial)),flush=True)
