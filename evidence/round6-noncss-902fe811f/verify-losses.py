#!/usr/bin/env python3
"""Cross-check all 292 historical rows against source DB payloads and producing raw receipts."""
import argparse,collections,hashlib,json,pathlib,sqlite3,zlib
P=pathlib.Path
p=argparse.ArgumentParser();p.add_argument('export',type=P);p.add_argument('--census',type=P,default=P.home()/'Library/Caches/lightningpanda/round6-noncss-measure/manifest-runs/census-902fe811f');p.add_argument('--final',action='store_true');args=p.parse_args()
root=args.census.parent;orig=root.parent/'loss-audit-batch2';inp=args.export/'historical-losses';load=lambda f:json.loads(f.read_bytes())
db=sqlite3.connect((root/'scores.sqlite').as_uri()+'?mode=ro&immutable=1',uri=True)
rows=load(inp/'rows.json');summary=load(inp/'summary.json'); configs=load(inp/'server-ports.json');expected=load(orig/'canonical-losses.json');basis={(t['url'],t['name'],t['occurrence']):t for t in load(orig/'losses.json')}
assert len(rows)==len(expected)==292 and summary['historicalLossesSha256']==hashlib.sha256((orig/'canonical-losses.json').read_bytes()).hexdigest()
assert all(all(t[k]==v for k,v in ref.items()) for t,ref in zip(rows,expected))
wanted={t['url'] for t in rows};payloads={};observed={r['url'] for r in rows if r['outcome']!='unmeasured-page'}
byfile=collections.defaultdict(set);oldmeta={};fromdb={}
for u in wanted:
 for l in ('base','combined2','chromeWpt'):
  loc,b=db.execute('select path,raw from result where label=? and url=?',(l,u)).fetchone();r=json.loads(zlib.decompress(b));byfile[loc].add(u);fromdb[loc,u]=r;payloads[l,u]=r
for filename,urls in byfile.items():
 got={r['url']:r for r in load(P(filename))['results'] if r['url'] in urls}
 assert set(got)==urls and all(r==fromdb[filename,u] for u,r in got.items()),filename
for k in load(root/'tasks.json'):
 urls=set(load(root/(k+'.json')))&observed
 if not urls:continue
 d=load(args.census/(k+'.json'));got={r['url']:r for r in d['results'] if r['url'] in urls};assert urls==got.keys();payloads.update({('current',u):v for u,v in got.items()})
def occurrence(row,name,n):
 a=[t for t in row['subtests'] if t['name']==name]
 return a[n-1]['status'] if n<=len(a) else None
s=collections.Counter();g=collections.defaultdict(collections.Counter)
for t in rows:
 u=t['url'];n=t['occurrence'];name=t['canonical'];first=t['base_name'];origrow=basis[u,first,n]
 assert origrow['classification']==t['classification'] and origrow['detail']==t['detail'] and origrow['combined2_page']==t['previousPage']
 assert occurrence(payloads['base',u],first,n)=='Pass'
 assert occurrence(payloads['combined2',u],t['head_name'] or name,n)==t['head_status']
 for label,origlabel in [('chrome','chromeWpt'),('current','current')]:
  v=t[label];row=payloads.get((origlabel,u))
  if row is None:
   assert v=={'page':None,'harness':None,'status':None,'name':None,'configSha256':None} and t['outcome']=='unmeasured-page';continue
  assert row['status']==v['page'] and row['harnessStatus']==v['harness'] and v['configSha256'] in configs
  assert occurrence(row,v['name'] or name,n)==v['status'],(label,u,n,name,v)
 outcome='unmeasured-page' if u not in observed else t['current']['status'] or 'missing-name'
 assert outcome==t['outcome'];s[outcome]+=1;g[t['classification']+' / '+t['detail']][outcome]+=1
assert s==summary['counts'] and g==summary['classification'] and len(wanted)==summary['affectedPages'] and len(observed)==summary['observedAffectedPages']
if args.final:assert summary['finishedPages']==30513 and len(observed)==len(wanted)
print(json.dumps({'historicalOccurrences':len(rows),'affectedUrls':len(wanted),'currentlyObservedUrls':len(observed),'historicalExportCutoff':summary['finishedPages'],'classifications':s,'rawOriginalFilesComparedWithDB':len(byfile)},indent=2))
