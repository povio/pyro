"""Freeze reviewed feature, prior-loss and sizing controls before measuring."""
import collections,gzip,hashlib,json
from pathlib import Path
p=Path(__file__).resolve().parent; d=p.parents[1]; c=p.parents[2]; prior=p.parent/'integration-52d8b7c/verified'; studio=p/'src/docs/validation/studio-css-e4ca26'; ref=c/'round6-reference-158/census/export-final';supp=d/'manifest-runs/census-cbdeb1193/css-supplement'
G=lambda f:json.loads(f.read_bytes())
def rows(f):
 with gzip.open(f,'rt') as stream:yield from map(json.loads,stream)
def put(f,v):f.write_text(json.dumps(v,indent=1)+'\n')
known=G(prior/'selected-urls.json')
safe=set(known);supp_url={}
for line in (supp/'raw/completed.jsonl').read_text().splitlines():
 r=json.loads(line);k=r['key']
 for u in G(supp/'selections'/(k+'.json')):supp_url[u]=k
safe.update(supp_url);native=set(G(ref/'studio-native-markers.json')['urls'])
native.add('websockets/back-forward-cache-with-open-websocket-connection-and-close-it-in-pagehide.window.html')
css=set(G(studio/'inlinebounds-controls.json'))|set(G(studio/'flexbounds-repeat.json'))
held={u:'prior Studio native marker' if u in native else 'outside earlier completed local controls' for u in css if u not in safe or u in native}
unresolved=set(G(supp/'selections/css-full-067.json'))
for u in held:
 if u in unresolved:held[u]='interrupted css-full-067; no attributable completed row'
cohorts={k:G(d/'template-navigator-combined'/(k+'.urls.json')) for k in ['focus','guard','navigator']}
assert set(cohorts['focus'])==set(G(d/'template-inert-documents/urls.json')) and set(cohorts['guard'])==set(G(d/'template-inert-documents/guard.json'))
features=set(sum(cohorts.values(),[]));loss={r[0] for r in rows(prior/'lost-names.jsonl.gz')}
gains=collections.Counter()
for u,n,i,b,h,cr in rows(prior/'named.jsonl.gz'):
 if h and h[0]=='Pass' and (not b or b[0]!='Pass'):gains[u]+=1
anchors=collections.defaultdict(list)
for u,cnt in sorted(gains.items(),key=lambda r:(-r[1],r[0])):
 area=u.split('/')[0]
 if known[u][2]=='non-css' and len(anchors[area])<2:anchors[area].append(u)
cohorts['production']=sorted((loss|set(sum(anchors.values(),[])))-features-css)
cohorts['css']=sorted(css-set(held))
assert not any(u in native for vs in cohorts.values() for u in vs)
assert len(set(sum(cohorts.values(),[])))==sum(map(len,cohorts.values()))
assert len(set(cohorts['production'])-safe)==0
(p/'selections').mkdir(exist_ok=True)
tasks=[]
for co,urls in cohorts.items():
 for ix in range(0,len(urls),80):
  stem=f'{co}-{ix//80:02d}';f=p/'selections'/(stem+'.json');put(f,urls[ix:ix+80])
  tasks.append(dict(key=stem,cohort=co,scope='css' if co=='css' else 'non-css',jobs=4 if co=='navigator' else 2,count=len(urls[ix:ix+80]),sha256=hashlib.sha256(f.read_bytes()).hexdigest()))
put(p/'tasks.json',tasks);put(p/'holds.json',held);put(p/'production-selection.json',dict(previousLossUrls=sorted(loss),highestGainExamples=anchors))
print({co:len(urls) for co,urls in cohorts.items()},held)
