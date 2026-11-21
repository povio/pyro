"""Standalone occurrence, status, diagnostic group, page transition and later-receipt readback."""
import collections,json
from pathlib import Path
p=Path(__file__).resolve().parent
load=lambda n:json.loads((p/n).read_bytes())
rows=load('losses.json');pages=load('page-observations.json');summary=load('summary.json');index=load('later-index.json');meta=load('source-receipts.json')
seen=set();h=collections.Counter();c=collections.Counter();groups=collections.Counter();stage=collections.Counter();later=collections.Counter();laterstatus=collections.Counter();laterurls=set();xor=collections.Counter();by=collections.defaultdict(collections.Counter)
for x in rows:
 u,n,i=x['url'],x['canonical'],x['occurrence'];key=u,n,i;assert key not in seen;seen.add(key)
 obs=x['observations'];a=obs['round7'];b=obs['previous'];z=obs['chrome'];assert b['status']=='Pass' and (a is None or a['status']!='Pass')
 pgs=pages[u];t=pgs['round7']['status'],pgs['round7']['harnessStatus'],pgs['previous']['status'],pgs['previous']['harnessStatus']
 stage[t]+=1;stat=a['status'] if a else 'missing';ch=z['status'] if z else 'missing';h[stat]+=1;c[ch]+=1;groups[x['observedGroup']]+=1;by[x['observedGroup']][ch]+=1
 for col in obs:assert col+'/'+pgs[col]['receipt'] in meta['raw']
 for y in x['laterReceiptMatches']:
  k=y['receipt'];r=index['matchedReceipts'][k];ident=index['sourceIdentities'][r['identity']]
  assert y['commit']==ident['source'] and ident.get('withdrawn') is None and ident['containsExactRound7']
  assert [u,n,i,y['status']] in r['observations'];later[k]+=1;laterstatus[y['status']]+=1;laterurls.add(key)
assert len(rows)==len(seen)==106 and len(pages)==21 and h==summary['newStatus'] and c==summary['chromeStatus'] and groups==summary['observedGroups']
assert later=={k:x['matchingOccurrences'] for k,x in index['matchedReceipts'].items()} and len(laterurls)==12 and set(laterstatus)=={'Fail'}
sig=load('signature-diagnostics.json');assert len(sig['rows'])==6
for x in sig['rows']:
 for label in ['round7','previous','chrome']:
  assert all(y['status']=='Pass' for y in x['matches'][label])
stats=dict(pageTransitions=[dict(new=list(t[:2]),old=list(t[2:]),count=n) for t,n in stage.items()],chromeByObservedGroup=by,laterSources=len({index['sourceIdentities'][x['identity']]['source'] for x in index['matchedReceipts'].values()}),laterReceipts=len(later),laterIndependentQA=sum(r['origin']=='independent QA' for r in index['matchedReceipts'].values()),laterDistinctOccurrences=len(laterurls),laterOccurrenceObservations=laterstatus,withoutLaterObservation=len(rows)-len(laterurls))
(p/'readback.json').write_text(json.dumps(stats,indent=1)+'\n')
print('Verified',len(seen),'losses on',len(pages),'URLs',stats)
