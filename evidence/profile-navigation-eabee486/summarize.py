from pathlib import Path
from collections import Counter
import json
out=Path(__file__).parent
rows=json.loads((out/'observations.json').read_text());summary=[]
assets={'Script','Stylesheet','Image','Font','Media'}
for p in rows:
 for n in p['navigations']:
  start=n['startHostMs']; events=[e for e in n['network'] if e['hostMs']<=n['observationEndHostMs']]
  cached={e.get('ordinal') for e in events if e['kind']=='cache' or e['kind']=='response' and e.get('fromDiskCache')}
  cached.discard(None)
  last=max((e['hostMs'] for e in events if e.get('type') in assets),default=None)
  cpu=lambda s:(s['userNs']+s['systemNs'])/1e6
  r={'sharedMiB':p['cacheMiB'],'privateMiB':64,'processOrdinal':p['processOrdinal'],'navigationOrdinal':n['navigationOrdinal'],'pid':p['pid'],'beforeSampleEpochMs':n['before']['epochMs'],'usableMs':n['usableHostMs']-start,'dclMs':n.get('dclHostMs',start)-start if 'dclHostMs' in n else None,'loadMs':n.get('loadHostMs',start)-start if 'loadHostMs' in n else None,'observedThroughMs':n['observationEndHostMs']-start,'postUsabilityHorizonMs':n['observationEndHostMs']-n['usableHostMs'],'lastObservedAssetEventMs':None if last is None else last-start,'requestTypes':dict(Counter(e.get('type','Unknown') for e in events if e['kind']=='request')),'cacheHitRequestCount':len(cached),'cdpDecodedDataBytes':sum(e.get('dataLength',0) for e in events if e['kind']=='data'),'terminalEncodedBodyBytes':sum(e.get('encodedDataLength',0) for e in events if e['kind']=='finished'),'failedCount':sum(e['kind']=='failed' for e in events),'browserCpuToUsableMs':cpu(n['usable'])-cpu(n['before']),'browserCpuToHorizonMs':cpu(n['end'])-cpu(n['before']),'rssBeforeMiB':n['before']['rssKiB']/1024,'rssUsableMiB':n['usable']['rssKiB']/1024,'rssEndMiB':n['end']['rssKiB']/1024,'footprintEndMiB':n['end']['footprintKiB']/1024,'domReadiness':n.get('domReady')}
  summary.append(r)
(out/'summary.json').write_text(json.dumps(summary,indent=2))
# Narrative qualifications are hand-maintained in RESULT/FOLLOWUP. Never
# overwrite those files when regenerating the numeric observations.
lines=['# Generated observer-on navigation table', '',
       'This is the original observer-on diagnostic, not a passive production baseline. '
       'Read [RESULT.md](RESULT.md) for collector/cache/epoch limits and '
       '[FOLLOWUP.md](FOLLOWUP.md) for observer-free confirmation and attribution. '
       'Original absolute navigation epochs are bounded, not exact.', '',
       '|Shared MiB|Process|Nav|Usable ms|DCL ms|Load ms|Browser CPU to usable ms|RSS usable MiB|Cache-hit requests|Last asset ms|',
       '|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|']
for r in summary:
 vals=[r[k] for k in ('sharedMiB','processOrdinal','navigationOrdinal','usableMs','dclMs','loadMs','browserCpuToUsableMs','rssUsableMiB','cacheHitRequestCount','lastObservedAssetEventMs')]
 lines.append('|'+ '|'.join('n/a' if x is None else f'{x:.1f}' if isinstance(x,float) else str(x) for x in vals)+'|')
(out/'GENERATED-TABLE.md').write_text('\n'.join(lines)+'\n')
print('Wrote summary.json and GENERATED-TABLE.md; RESULT.md and FOLLOWUP.md preserved.')
