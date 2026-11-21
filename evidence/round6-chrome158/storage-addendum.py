"""Passive addendum to the immutable joint selections. Never launches a runtime."""
import collections,gzip,hashlib,json,pathlib,sys
p=pathlib.Path(__file__).resolve().parent;out=pathlib.Path(sys.argv[1]);r=p.parents[1]/'round6-noncss-measure/merged-aa0268';a=r/'accepted158';D=lambda x:json.loads(x.read_bytes());sha=lambda f:hashlib.sha256(f.read_bytes()).hexdigest()
urls=set(D(r/'wasm-storage-alignment/urls.json'));core=set(D(p/'plans/shared-noncss.urls.json'))
assert len(urls)==5 and not (urls&core) and all(u.startswith('wasm/serialization/module/') for u in urls)
sources={'cbdeb':a/'baseline-extra/wasm-storage-supplement-000.json','b57':a/'raw/wasm-storage-supplement-000.json','chrome158':r/'wasm-storage-alignment/chrome158.json'}
names=collections.defaultdict(dict);pages=collections.defaultdict(dict);proof={};raws={}
for label,f in sources.items():
 doc=D(f);assert doc['wptRevision']=='a421f2fea5eb7f91e0db399e44b0fb0779ee1603' and doc['jobs']==4 and not doc['restarts'] and not doc['watchdogKills']
 assert doc['manifestInventory'] and doc['inventoryPlan']['viewport']=={'width':800,'height':600,'deviceScaleFactor':1}
 assert {row['url'] for row in doc['results']}==urls
 proof[label]={'artifact':str(f.relative_to(r)),'sha256':sha(f),'jobs':4,'restarts':0,'watchdogKills':0,'source':'separately reviewed module serialization/storage controls'}
 doc['bin']=pathlib.Path(doc['bin']).name;raws[label]=doc
 for row in doc['results']:
  u=row['url'];items=[];oc=collections.Counter()
  for sub in row['subtests']:
   n=sub['name'];oc[n]+=1;key=n,oc[n];names[u].setdefault(key,{})[label]=sub['status'];items.append(sub['status'])
  pages[u][label]=[row['status'],row['harnessStatus'],row['status']=='complete' and row['harnessStatus']=='OK' and bool(items) and all(v=='Pass' for v in items)]
assert sha(a/'candidate')=='296b3d4f9775c92d98d3bd7aa898efc68b1a18e90e591af5a428ad8decd4e9bf'
assert sha(r/'batch/visible')=='81d12af223643b8d5c1903fd528a36a5f2ff3040855f998d7bc5e546805b93b2'
assert sum(len(ns) for ns in names.values())==12
flat={e:collections.Counter() for e in sources}
for u,byName in names.items():
 for obs in byName.values():
  for e,s in obs.items():flat[e][s]+=1
summary={'kind':'separate storage-only addendum; excluded from the original shared selections and totals','versions':{'cbdeb':'cbdeb11931f74c3de4417a25fb9103f1df84d54e','b57':'b57c35dd45d54d6781aafcb2c8242c9ad9b26e04','chrome158':'158.0.8127.37'},'urls':sorted(urls),'inputProvenance':proof,'binarySHA256':{'cbdeb':sha(r/'batch/visible'),'b57':sha(a/'candidate'),'chrome158':D(p/'reference-identity.json')['executableSHA256']},'selectedInMainInventory':0,'pages':pages,'named':[ [u,*k,v] for u,ns in sorted(names.items()) for k,v in sorted(ns.items()) ],'counts':flat,'strict':{e:sum(row[e][2] for row in pages.values()) for e in sources}}
(out/'storage-only-addendum.json').write_text(json.dumps(summary,indent=2)+'\n')
with gzip.open(out/'storage-only-addendum-receipts.json.gz','wt',compresslevel=4) as f:json.dump(raws,f,separators=(',',':'))
print({e:(dict(v),summary['strict'][e]) for e,v in flat.items()})
