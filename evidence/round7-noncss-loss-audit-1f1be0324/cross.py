"""Offline exact-occurrence index across local round-seven testharness receipts and verified LP source identities."""
import collections,datetime,gzip,hashlib,json,os,re,subprocess,sys
from pathlib import Path
p=Path(__file__).resolve().parent;cache=p.parents[1];reference='1f1be032436bfeee9f48cdb5acbc0f71d21d5af9'
def sha(f):
 with f.open('rb') as s:return hashlib.file_digest(s,'sha256').hexdigest()
def load(f):return json.loads(f.read_bytes())
def save(n,x):(p/n).write_text(json.dumps(x,indent=1,ensure_ascii=True)+'\n')
def git(*a):return subprocess.check_output(['git',*a],text=True).strip()
def ancestor(a,b):return subprocess.run(['git','merge-base','--is-ancestor',a,b]).returncode==0
baseline_time=int(git('show','-s','--format=%ct',reference));loss=load(p/'losses.json')
byurl=collections.defaultdict(dict)
for r in loss:r['laterReceiptMatches']=[];byurl[r['url']][r['canonical'],r['occurrence']]=r
branches=git('for-each-ref','--format=%(objectname) %(refname:short)','refs/remotes/origin/').splitlines()
identities={};scanned={};matches={};excluded=collections.defaultdict(list)
for root in sorted(cache.glob('round7-*')):
 for path,dirs,files in os.walk(root):
  dirs[:]=[n for n in dirs if n not in ('target','src','crates','packages','vendor','selections','runner','runner-src','node_modules','.git','census-1f1be0324','loss-audit-1f1be0324','verified-ports','build-target') and not (Path(path)/n/'.git').exists()]
  for n in sorted(files):
   if not n.endswith('.json'):continue
   f=Path(path)/n;key=str(f.relative_to(cache))
   with f.open('rb') as s:prefix=s.read(1500)
   if b'"bin"' not in prefix:continue
   try:doc=load(f)
   except (ValueError,UnicodeDecodeError):continue
   if not isinstance(doc,dict) or not isinstance(doc.get('results'),list) or not isinstance(doc.get('bin'),str):continue
   found=[r for r in doc['results'] if isinstance(r,dict) and r.get('url') in byurl];scanned[key]=dict(rows=len(doc['results']),overlap=len(found),profile=dict(jobs=doc.get('jobs'),server=doc.get('server'),wpt=doc.get('wptRevision')))
   if not found:continue
   if doc.get('wptRevision')!='a421f2fea5eb7f91e0db399e44b0fb0779ee1603':excluded['missing-or-different-pinned-profile'].append(key);continue
   bf=Path(doc['bin'])
   if bf.name=='launch-158':excluded['chrome-other-profile'].append(key);continue
   # Every later matching occurrence in this scan is an ordinary editing test;
   # its literal names contain neither URL authorities nor normalized ports.
   ident=str(bf.relative_to(cache)) if bf.is_relative_to(cache) else ''
   if ident not in identities:
    prov=bf.parent/(bf.name+'-provenance.txt')
    if not prov.is_file():excluded['no-build-provenance'].append(key);continue
    lines=prov.read_text().splitlines();source=lines[0];tree=lines[1];assert re.fullmatch('[0-9a-f]{40}',source) and git('rev-parse',source+'^{tree}')==tree
    binary_sha=sha(bf);assert any(l.split()[:1]==[binary_sha] for l in lines) and any(l.startswith('3deb71f8e7d14daf174e35340a4ff0c1900b0e9f576503adeb5facbb1201f029 ') for l in lines)
    tips=git('for-each-ref','--format=%(objectname) %(refname:short)','--contains',source,'refs/remotes/origin/testharness/','refs/remotes/origin/topic/').splitlines()
    identities[ident]=dict(source=source,tree=tree,buildReceipt=sha(prov),binarySHA256=binary_sha,sourceTime=git('show','-s','--format=%cI',source),sourceEpoch=int(git('show','-s','--format=%ct',source)),containsExactRound7=ancestor(reference,source),publishedTips=tips)
   data=identities[ident]
   if data['binarySHA256']=='543a7b85e3ecd8b99400d8c0c6861ff4756621b106d86407ca9d9098ca9b0d4d' and data['source']!=reference:
    data['withdrawn']='Known misattributed cached binary identical to frozen 1f1be0324; correct e1a29d483 rebuild c299b0878af728385ff3d37509fe9512170cdf579cc9e335443f1a2cbc2df2bd kept separately.'
    excluded['withdrawn-misattributed-binary'].append(key);continue
   if data['sourceEpoch']<=baseline_time:excluded['source-predates-census-head'].append(key);continue
   matched=0;rows=[]
   for r in found:
    counts=collections.Counter()
    for s in r.get('subtests',[]):
     name=s['name'];counts[name]+=1;i=counts[name];target=byurl[r['url']].get((name,i))
     if target is None:continue
     assert '%' not in name
     obs=dict(receipt=key,commit=data['source'],status=s['status'],sourceName=name,pageStatus=r['status'],harnessStatus=r.get('harnessStatus'),msg=s.get('msg'))
     target['laterReceiptMatches'].append(obs);rows.append([r['url'],name,i,s['status']]);matched+=1
   if matched:matches[key]=dict(sha256=sha(f),identity=ident,origin='independent QA' if key.startswith('round7-qa/') else 'feature lane',profile={k:doc.get(k) for k in ['jobs','server','http2','testdriver','manifestInventory','wptRevision','reuseContext']},matchingOccurrences=matched,observations=rows)
   else:excluded['overlapping-url-without-same-name'].append(key)
save('losses.json',loss);save('later-index.json',dict(cutoffUTC=datetime.datetime.now(datetime.timezone.utc).isoformat(),scannedRoots=['round7-*'],scanned=scanned,excluded=excluded,sourceIdentities=identities,matchedReceipts=matches))
print('scanned',len(scanned),'matched receipts',len(matches),'occurrences',sum(bool(r['laterReceiptMatches']) for r in loss),'matches',sum(len(r['laterReceiptMatches']) for r in loss),'states',collections.Counter(obs['status'] for r in loss for obs in r['laterReceiptMatches']))
