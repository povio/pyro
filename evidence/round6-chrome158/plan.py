import json,gzip,hashlib,re,collections,pathlib
D=pathlib.Path(__file__).resolve().parent;cache=D.parents[1];r=cache/'round6-noncss-measure';root=r/'manifest-runs';out=D/'plans';inv=cache/'round6-inventory';load=lambda f:json.loads(f.read_bytes())
pin='a421f2fea5eb7f91e0db399e44b0fb0779ee1603'
def save(k,x): (out/k).write_text(json.dumps(x,indent=1)+'\n')
def archive(fname):
 dec=json.JSONDecoder()
 with gzip.open(fname,'rt') as f:
  buf=f.read(1<<17);m=re.match(r'\s*\{\s*"results"\s*:\s*\[',buf);assert m;buf=buf[m.end():]
  while True:
   buf=buf.lstrip(' \t\r\n,')
   if buf.startswith(']'):return
   try:row,end=dec.raw_decode(buf)
   except ValueError:
    piece=f.read(1<<17)
    if not piece:raise
    buf+=piece;continue
   yield row;buf=buf[end:]
non=load(root/'final-official-inventory.json');css=load(out/'css-inventory.json')
records={x['url']:x for part in (non,css) for x in part['eligible']+part['inventoryOnly']};assert len(records)==39989 and len(non['eligible'])+len(non['inventoryOnly'])==31827
scope={};eligible902=set();reasons=collections.defaultdict(list)
for l in gzip.open(root/'census-902fe811f/comparison/inventory.jsonl.gz','rt'):
 u,source,blob,env,reason,*_=json.loads(l)
 if reason:reasons[u].append(['902fe811f inventory',reason])
 else:eligible902.add(u)
assert len(eligible902)==30513
guard=load(r/'merged-aa0268/batch/readback/evidence/provenance.json')
heads=r/'merged-aa0268/batch/readback/evidence'
for ln in gzip.open(heads/'variant-inventory.jsonl.gz','rt'):
 cohort,u,env,protocol,src,blob=json.loads(ln);scope.setdefault(u,[]).append(cohort)
assert len(scope)==4038
for x in css['inventoryOnly']:reasons[x['url']].append(['CSS manifest inventory',x['reason']])
for u in records:
 if u.startswith('css/') and u not in scope:reasons[u].append(['separate CSS execution lane','inventoried here; assigned to Studio'])
# Only exact window URLs positively verified earlier as fully passing synchronous JS API controls.
w=load(r/'wasm-ordinary/ordinary-safe-e4ca.json');wp=load(r/'wasm-ordinary/ordinary-safe-chrome.json')
checks={z['url'] for z in wp['results'] if z['status']=='complete' and z['subtests'] and all(t['status']=='Pass' for t in z['subtests'])}
healthy={z['url'] for z in w['results'] if z['url'] in checks and z['status']=='complete' and z['subtests'] and all(t['status']=='Pass' for t in z['subtests'])}
assert len(healthy)==9
for u in healthy:
 reasons[u]=[x for x in reasons[u] if x[1]!='Wasm runtime withheld']
# Exact local recorded native hang/crash markers and unexplained browser process exits.
for u,items in load(out/'native-observed.json').items():
 if u in records:reasons[u].extend([['prior local native event',x[:2]] for x in items])
ambiguous=[]
for z in load(out/'restart-receipts.json'):
 f=pathlib.Path(z['receipt']);doc=load(f)
 h=sum(r['status']=='hang' for r in doc['results'])
 if z['restarts']!=h or z['watchdog']:
  key='/'.join(f.parts[-2:]);ambiguous.append({'receipt':key,'restarts':z['restarts'],'guardedHangCount':h,'watchdogKills':z['watchdog'],'possiblyAffectedUrls':len(z['urls'])})
  for u in z['urls']:
   if u in records:reasons[u].append(['native termination not attributable to one URL',key])
# Retain CRASH from published Chromium runs; an ordinary harness TIMEOUT is not a native hang.
for ch in ('stable','canary'):
 for row in archive(inv/f'report-{ch}.download'):
  u=row['test'].lstrip('/')
  if row['status']=='CRASH' and u in records:reasons[u].append(['public Chrome '+ch,'CRASH'])
# Authoritative historical checked-in statuses for this WPT revision, including the CSS lane.
repo=r/'manifest-runner/bench/wpt'
for fp in list((repo/'full/testharness').glob('*.json'))+[repo/'baseline.json',repo/'baseline-linux.json']:
 if not fp.is_file():continue
 doc=load(fp)
 if doc.get('wpt',{}).get('revision')!=pin:continue
 for u,v in doc.get('tests',{}).items():
  if u in records and v.get('status') in ('hang','crash','panic'):reasons[u].append(['checked-in LP '+str(fp.relative_to(repo)),v['status']])
# Publicly selected CSS guard and other positive controls are never implicitly added outside their reviewed scope.
sel={u for u in records if not reasons[u]}
assert sel <= eligible902|healthy|{u for u in scope if u.startswith('css/')}
prior=sorted(u for u in sel if u in scope);nonSel=sorted(u for u in sel if not u.startswith('css/'));cssSel=sorted(u for u in sel if u.startswith('css/'))
save('shared-noncss.urls.json',nonSel);save('css-guard.urls.json',cssSel);save('priority.urls.json',prior);save('ambiguous-native-receipts.json',ambiguous)
with gzip.open(out/'official-inventory.jsonl.gz','wt') as f:
 for u in sorted(records):
  v=records[u];print(json.dumps(dict(url=u,source=v['source'],blob=v['blob'],environment=v['environment'],scope='CSS' if u.startswith('css/') else 'non-CSS',guard=scope.get(u,[]),eligible=u in sel,withheld=reasons[u]),separators=(',',':')),file=f)
summary={'wpt':pin,'manifestSHA256':non['manifestSha256'],'nonCSS':31827,'previousReviewedSelected':30513,'CSS':8162,'sharedNonCSS':len(nonSel),'localCSSGuard':len(cssSel),'selectedPriority':len(prior),'wasmControlCandidates':sorted(healthy),'eligibleWasmControls':sorted(healthy&sel),'withheldOldNonCSS':len(eligible902-sel),'driversAreInfrastructure':len(load(out/'driver-only-interruptions.json')),'reasons':collections.Counter(reason[0] for u in records for reason in reasons[u]),'ambiguousNativeReceipts':ambiguous,'selectionSHA256':{k:hashlib.sha256((out/k).read_bytes()).hexdigest() for k in ('shared-noncss.urls.json','css-guard.urls.json','priority.urls.json')}}
save('summary.json',summary);print(json.dumps(summary))
