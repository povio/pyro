"""Attach distinct MacBook and Studio coverage fields to every official URL."""
import gzip,itertools,json,pathlib,sys,collections
p=pathlib.Path(sys.argv[1])
def records(f):
 with gzip.open(p/f,'rt') as r:
  for line in r:yield json.loads(line)
def dump(x):return json.dumps(x,separators=(',',':'))
studio={r[0]:r for r in records('studio-css-pages.jsonl.gz')};assert len(studio)==8162
ct=collections.defaultdict(collections.Counter)
with gzip.open(p/'cross-machine-inventory.jsonl.gz','wt') as f:
 for u,source,blob,env,scope,guards,state,reasons,k,local in records('official-pages.jsonl.gz'):
  r=studio.get(u);s=r[5] if r else None
  if r is not None:assert r[1:4]==[source,blob,env] and r[6]==local
  why=r[8] if r and s is None else None
  label=('both' if s and local else 'only-local' if local else 'only-Studio' if s else 'not-observed')
  ct[scope][label]+=1
  if label=='not-observed':
   assert state!='observed' and (scope=='non-CSS' or why)
  print(dump([u,source,blob,env,scope,label,{'page':local,'shard':k,'status':state,'reasons':reasons},{'page':s,'sourceCohort':r[4] if r else None,'reason':why if r else 'outside Studio CSS reference'}]),file=f)
(p/'cross-machine-summary.json').write_text(json.dumps(dict(scope=ct,wpt='a421f2fea5eb7f91e0db399e44b0fb0779ee1603',stable='158.0.8127.37',total=sum(sum(n.values()) for n in ct.values()),namedSets='reference-names.jsonl.gz and studio-css-names.jsonl.gz keep separate sources'),indent=2)+'\n')
s=json.loads((p/'studio-compatibility.json').read_text());paired=s['paired'];o=p/'README.md';text=o.read_text()
msg=f"""## Cross-machine CSS compatibility

The Studio main and 41-URL WPT-certificate supplement also used official stable 158.0.8127.37 on macOS 27.2.1 and pinned WPT. Studio recorded {s['studioRecorded']['observed']:,}/8,162 CSS URLs, {s['studioRecorded']['full']:,} fully passing and {s['studioRecorded']['namedPass']:,} named passes. Thirty-nine crash/security fixtures remain inventoried without execution. The 1,091 locally observed CSS URLs all have Studio records. The 7,032 Studio-only CSS URLs retain their Studio results and their local coverage reason. No URL or named results are pooled on an overlap.

On those {paired['matched']:,} shared URLs the two machines each recorded {paired['localPass']:,} passes. The intersection contains {paired['commonPass']:,} named passes; {paired['onlyStudioPass']} are exclusive to Studio and {paired['onlyLocalPass']} to this Mac. Studio fully passed {paired['studioFull']} of the overlapping URLs and this Mac fully passed {paired['localFull']}. All 41 supplemental URLs matched on named and strict-page pass sets.

Studio's original 8,082-URL run used headless CDP in disposable profiles and 16 disjoint shards, with extensions/background component extensions disabled in four shards following a driver attachment failure. Its separate service-worker supplement used the WPT leaf SPKI. The original recorded configuration does not establish that Studio used this Mac's popup preference, per-feature WebNN setting, native GPU choice, exact Playwright version, concurrency or complete normalized-port mapping. There were no experimental web-platform flags. The paired difference is observational and does not assign a cause to the profile differences. [Studio main reference](https://github.com/povio/pyro/tree/fd629a078f697139cf140adbb2a315e90037bb25/docs/validation/studio-css-e4ca26) · [Studio supplemental reference](https://github.com/povio/pyro/tree/8e84143126a3335af41906350060477c69751616/docs/validation/studio-css-e4ca26/color-gap-controls.md)

cross-machine-inventory.jsonl.gz enumerates all 39,989 variants with separate local and Studio slots. studio-css-pages.jsonl.gz and studio-css-names.jsonl.gz preserve actual Studio harness status, raw named result, one-based occurrence and the parallel MacBook observation when present. Studio file hashes, source commits and exact missing rows are in studio-compatibility.json.

"""
text=text.replace('## Reproduction and interpretation',msg+'## Reproduction and interpretation');o.write_text(text)
