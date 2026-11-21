"""Join each immutable census loss to all three originating raw page and named diagnostics."""
import collections,functools,gzip,hashlib,json,re,subprocess,sys
from pathlib import Path
p=Path(__file__).resolve().parent
cache=Path.home()/'Library/Caches/lightningpanda';d=cache/'round6-noncss-measure'
new=cache/'round7-local/census-1f1be0324';old=d/'manifest-runs/census-8e7144b7e'
ref=cache/'round6-reference-158/census'
sys.path.insert(0,str(d/'manifest-runs/census-902fe811f'))
from wpt_ports import DEFAULTS,canonical_name
load=lambda f:json.loads(f.read_bytes())
def sha(f):
 with f.open('rb') as s:return hashlib.file_digest(s,'sha256').hexdigest()
def save(n,v):(p/n).write_text(json.dumps(v,ensure_ascii=True,indent=1)+'\n')
def rows(f):
 with gzip.open(f,'rt') as s:yield from map(json.loads,s)
def git(*a):return subprocess.check_output(['git',*a],text=True).strip()
commit='01f49f43da24a20ecb20d99c967fc5de54272120';folder='evidence/round7-noncss-census-1f1be0324'
inputs={}
for n in ['prior-pass-changes.jsonl.gz','pages.jsonl.gz','raw-provenance.json','previous-census-provenance.json','summary.json']:
 f=new/'evidence'/n;assert git('rev-parse',commit+':'+folder+'/'+n)==git('hash-object',str(f))
 inputs[n]=sha(f)
changes=[r for r in rows(new/'evidence/prior-pass-changes.jsonl.gz') if r[3]=='loss']
urls={r[0] for r in changes};pages={r[0]:r for r in rows(new/'evidence/pages.jsonl.gz') if r[0] in urls}
meta_new=load(new/'evidence/raw-provenance.json');meta_old=load(new/'evidence/previous-census-provenance.json')
def get_raw(label,k):
 if label=='round7':
  dir=new/'raw';meta=meta_new[k];h=meta['receiptSHA256'];loghash=meta['logSHA256']
 else:
  lane,k2=k.split('/',1);dir={'chrome':ref/'raw','remaining':old/'raw','guard':d/'merged-aa0268/integration-52d8b7c/raw-head'}[lane];meta=meta_old[k];h=meta['receipt'];loghash=meta['log'];k=k2
 f=dir/(k+'.json');assert sha(f)==h and sha(f.with_suffix('.log'))==loghash
 raw=load(f);assert raw['jobs']==4 and raw['http2'] and raw['testdriver'] and raw['wptRevision']=='a421f2fea5eb7f91e0db399e44b0fb0779ee1603'
 m={}
 for role,ps in meta['ports'].items():
  if role in DEFAULTS:
   for a,b in zip(ps,DEFAULTS[role]):m[('https' if role=='h2' else role.split('-')[0],a)]=b
 return {r['url']:r for r in raw['results']},m,dict(sha256=h,logSHA256=loghash,configSHA256=meta.get('configSHA256',meta.get('config')),jobs=4,restarts=raw['restarts'],watchdogKills=raw['watchdogKills'])
get_raw=functools.lru_cache(maxsize=4)(get_raw)
groups=collections.Counter();states=collections.Counter();bychrome=collections.Counter()
out=[];pg={};names={};provenance={}
for u in sorted(urls):
 page=pages[u];pg[u]={};names[u]={}
 for label,k,expected in zip(['round7','chrome','previous'],page[4],page[1:4]):
  raw,m,meta=get_raw(label,k);r=raw[u];ct=collections.Counter();ix={}
  for sub in r['subtests']:
   n=canonical_name(sub['name'],m);ct[n]+=1;ix[n,ct[n]]=sub
  names[u][label]=ix
  assert expected[:2]==[r['status'],r['harnessStatus']] and len(ix)==expected[4]
  pg[u][label]=dict(receipt=k,status=r['status'],harnessStatus=r['harnessStatus'],fullyPassing=expected[2],passing=expected[3],reported=expected[4],elapsedMs=r.get('ms'),error=r.get('error'),diagnosticSubtests=[s for s in r['subtests'] if s['name'].startswith('html5lib-runner:')])
  provenance[label+'/'+k]=meta
def classify(u,n,pg,sub):
 if u.startswith('subresource-integrity/'):return 'live-signature-in-name'
 if u.startswith('html/syntax/parsing/'):return 'fixture-fetch-502'
 if pg['status']=='timeout':return 'page-timeout-no-loss-name'
 if (pg['error'] or '').startswith('goto: net::ERR_PROXY_CONNECTION_FAILED'):return 'proxy-connection-failed'
 if (pg['error'] or '').startswith('connectOverCDP: connect EADDRNOTAVAIL'):return 'cdp-local-address-unavailable'
 if 'source image could not be decoded' in ((pg['error'] or '')+' '+(sub or {}).get('msg','')):return 'source-image-decode-rejection'
 if u.startswith('editing/'):return 'plaintext-formatting-assertion'
 if 'moving-between-documents' in u:return 'moved-script-event-assertion'
 if u.startswith('web-animations/'):return 'timeline-clock-assertion'
 if u.startswith('mixed-content/'):return 'blocked-vs-upgraded-assertion'
 if u.startswith('preload/'):return 'resource-count-assertion'
 if u.startswith('fetch/api/basic/'):return 'response-status-assertion'
 if 'canvas-display-p3-drawImage.https.html' in u:return 'pixel-value-assertion'
 raise ValueError(u)
for u,n,i,sg,h,c,b,_ in changes:
 obs={}
 for key,outcome in zip(['round7','chrome','previous'],[h,c,b]):
  sub=names[u][key].get((n,i));assert ([sub['status'],sub['name']] if sub else None)==outcome
  obs[key]=sub
 group=classify(u,n,pg[u]['round7'],obs['round7'])
 groups[group]+=1;states[h[0] if h else 'missing']+=1;bychrome[c[0] if c else 'missing']+=1
 out.append(dict(url=u,canonical=n,occurrence=i,observations=obs,observedGroup=group,laterReceiptMatches=[]))
# Diagnostic-only cross-name projection. Do not alias these occurrences in pass counts.
u=next(u for u in urls if u.startswith('subresource-integrity/'))
signature_r=re.compile(r'signature=:[A-Za-z0-9+/=]+:')
signature=[]
for rec in out:
 if rec['url']!=u:continue
 n=rec['canonical'];skel=signature_r.sub('signature=:[runtime signature]:',n)
 matches={label:[s for (key,i),s in dic.items() if signature_r.sub('signature=:[runtime signature]:',key)==skel] for label,dic in names[u].items()}
 assert all(matches[k] for k in matches)
 signature.append(dict(original=n,structuralGroup=skel,matches=matches))
src=cache/'wptci/wpt-full/subresource-integrity/signatures/tentative/authority.https.window.js'
save('signature-diagnostics.json',dict(source='subresource-integrity/signatures/tentative/authority.https.window.js',blob=git('-C',str(cache/'wptci/wpt-full'),'hash-object',str(src)),ranges=[[3,8],[33,43],[63,85]],rows=signature))
save('source-receipts.json',dict(census=commit,exportHashes=inputs,raw=provenance));save('page-observations.json',pg);save('losses.json',out);save('summary.json',dict(namedLosses=len(out),urls=len(urls),newStatus=states,chromeStatus=bychrome,observedGroups=groups))
