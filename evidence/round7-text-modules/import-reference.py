from pathlib import Path
import gzip,hashlib,json,collections
w=Path(__file__).parent;ref=w.parent/'round6-reference-158/census';m=json.load(open(w/'signed158-census-reference-map.json')); ps={r[0]:r for r in m['pages']}; ns=collections.defaultdict(list)
with gzip.open(ref/'export-final/reference-names.jsonl.gz','rb') as f:
 for line in f:
  if not line.startswith(b'["html/semantics/scripting-1/the-script-element/'):continue
  row=json.loads(line)
  if row[0] in ps:ns[row[0]].append(row)
out=[];receipts={}
for k,urls in m['shards'].items():
 f=ref/'raw'/(k+'.json');raw=f.read_bytes();run=json.loads(raw);assert run['wptRevision']==m['wpt'] and run['jobs']==4 and run['http2'] and run['testdriver'];receipts[k]=hashlib.sha256(raw).hexdigest()
 for u in urls:
  p=ps[u][9]; names=ns[u];assert len(names)==p[3] and sum(r[3]=='Pass' for r in names)==p[4]
  out.append(dict(url=u,status=p[0],harnessStatus=p[1],subtests=[dict(name=r[4],status=r[3]) for r in names]))
assert {p['url'] for p in out}==set(json.load(open(w/'urls.json')))
(w/'chrome158.json').write_text(json.dumps(dict(results=sorted(out,key=lambda p:p['url']),restarts=None,watchdogKills=None,referenceSource='signed census at ee24e07f6'),separators=(',',':')))
(w/'chrome-receipt-hashes.json').write_text(json.dumps(receipts,indent=2));print(len(out),sum(len(p['subtests']) for p in out),sum(t['status']=='Pass' for p in out for t in p['subtests']))
