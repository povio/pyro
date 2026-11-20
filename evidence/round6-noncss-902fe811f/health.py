"""Separate recorded process interruptions from ordinary WPT harness outcomes."""
from pathlib import Path
from collections import Counter
import gzip,hashlib,json
p=Path(__file__).resolve().parent
history=set(json.loads((p/'original-runner-shards.json').read_bytes()))
totals=Counter();events=[];selections={};checks=[]
for key,n in map(json.loads,(p/'completed.jsonl').read_text().splitlines()):
 d=json.loads((p/(key+'.json')).read_bytes())
 selected=json.loads((p/'selections'/(key+'.json')).read_bytes())
 assert set(selected)=={r['url'] for r in d['results']} and len(selected)==n
 selections[key]=selected
 sha=lambda f:hashlib.sha256(f.read_bytes()).hexdigest()
 old=key in history
 checks.append(dict(shard=key,originalRunner=old,reviewedHistoricalSkip=key.startswith('legacy-reviewed-'),rawReceiptSHA256=sha(p/(key+'.json')),selectionSHA256=sha(p/'selections'/(key+'.json')),logSHA256=sha(p/(key+'.log')),restarts=d['restarts'],watchdogKills=d['watchdogKills']))
 totals['originalUrls' if old else 'noReplayUrls']+=n
 totals['restarts']+=d['restarts'];totals['watchdogKills']+=d['watchdogKills']
 if old:assert not d['restarts'] and not d['watchdogKills'],key
 for r in d['results']:
  if r['status'] in ('crash','hang') or 'observation withheld without replay' in str(r.get('error')):
   events.append(dict(url=r['url'],outerStatus=r['status'],namedReported=len(r['subtests']),shard=key))
result=dict(totals=totals,interruptions=events,priorExcluded=json.loads((p/'coverage.json').read_bytes())['withheld'])
(p/'comparison/process-coverage.json').write_text(json.dumps(result,indent=2))
with gzip.open(p/'comparison/shard-selections.json.gz','wt') as f:json.dump(selections,f,separators=(',',':'))
(p/'comparison/shard-provenance.json').write_text(json.dumps(checks,indent=1))
print(totals)
