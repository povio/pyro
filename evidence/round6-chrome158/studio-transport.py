import gzip,hashlib,json,pathlib,re,subprocess,sys,collections
p=pathlib.Path(__file__).resolve().parent;out=pathlib.Path(sys.argv[1]);ps=p/'studio-source';read=lambda n:json.loads(gzip.open(ps/n,'rb').read());main=read('outcomes-stable.json.gz');supp={r['url']:r for r in read('chrome-wpt-cert-supplement.json.gz')}
inv={r['url']:r for l in gzip.open(p/'plans/official-inventory.jsonl.gz','rt') if (r:=json.loads(l))['scope']=='CSS'}
data={'main':'fd629a078f697139cf140adbb2a315e90037bb25','supplement':'8e84143126a3335af41906350060477c69751616'};proof={};ct=collections.defaultdict(collections.Counter)
for k,c in data.items():
 f='bench/wpt/run.mjs';oid=subprocess.check_output(['git','rev-parse',c+':'+f],text=True).strip()
 s=subprocess.check_output(['git','cat-file','blob',oid]);assert rb'/\.(https|serviceworker)\./.test(name)' in s
 proof[k]={'commit':c,'path':f,'blob':oid,'sha256':hashlib.sha256(s).hexdigest(),'method':'baseFor: suffix .https. or .serviceworker. => https, other => http; .www. => www.web-platform.test'}
with gzip.open(out/'studio-css-route-provenance.jsonl.gz','wt') as o:
 for u,r in sorted(inv.items()):
  cohort='main' if u in main else 'supplement' if u in supp else None
  name=re.split('[?#]',u,1)[0].rsplit('/',1)[-1];scheme=('https' if re.search(r'\.(https|serviceworker)\.',name) else 'http') if cohort else None
  host=('www.web-platform.test' if '.www.' in name else 'web-platform.test') if cohort else None
  observed=(supp[u].get('protocol') if u in supp else None)
  if cohort:ct[cohort][scheme]+=1
  print(json.dumps([u,r['source'],r['blob'],r['environment'],cohort,scheme,host,observed],separators=(',',':')),file=o)
(out/'studio-css-route-summary.json').write_text(json.dumps({'source':proof,'counts':ct,'schema':['url','source','gitBlob','manifestEnvironment','StudioCohort','schemeDerivedFromRunner','hostDerivedFromRunner','verbatimProtocolField'],'note':'Both archived controls used wptserve. Route fields are derived from the exact runner source at each data commit; neither original compact outcomes nor the supplemental raw records retained per-page negotiated protocol, session port, or ALPN.'},indent=2)+'\n')
print(dict(ct))
