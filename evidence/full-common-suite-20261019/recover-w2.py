from pathlib import Path
import json,hashlib
b=Path('/private/tmp/lp-w2cov');out=Path('/private/tmp/lp-suite-20261019');entries=[]
for p in sorted(b.glob('w2-*/*/lightpanda.json')):
 if 'upstream'in p.parts:continue
 try:d=json.loads(p.read_text());meta=json.loads((p.parent/'metadata.json').read_text())
 except Exception:continue
 if meta.get('tos_commit')!='f9fde4aa0dce7d1d7a1ffebffe3306399973cd80':continue
 def visit(s,path):
  title=s.get('title','');new=path+([title]if title and not title.endswith('.e2e.ts')else[])
  for sp in s.get('specs',[]):
   for t in sp.get('tests',[]):
    entries.append({'batch':p.parts[-3],'file':sp.get('file',s.get('file')),'titlePath':new+[sp['title']],'reported_status':t.get('status'),'expected_status':t.get('expectedStatus'),'attempt_statuses':[r.get('status')for r in t.get('results',[])],'retries':[r.get('retry')for r in t.get('results',[])],'start':d.get('stats',{}).get('startTime'),'engine':meta.get('browser_commit'),'engine_sha256':meta.get('browser_sha256'),'report':str(p),'run_slug':meta.get('run_slug')})
  for sub in s.get('suites',[]):visit(sub,new)
 for s in d.get('suites',[]):visit(s,[])
latest={}
for r in sorted(entries,key=lambda r:r['start']or''):latest[(r['file'],tuple(r['titlePath']))]=r
unresolved=[r for r in latest.values()if r['reported_status']!='expected'or r['expected_status']!='passed']
(out/'historical-w2-20-32.json').write_text(json.dumps({'scope':'historical retained report inventory only; later targeted repairs not applied; not current-main/final suite results','all_observed_runs':entries,'latest_per_case':list(latest.values()),'latest_not_passed':unresolved},indent=2)+'\n')
print('records',len(entries),'latestcases',len(latest),'notpassed',len(unresolved))
for r in unresolved:print(r['batch'],r['reported_status'],r['file'],' / '.join(r['titlePath']),r['engine'],Path(r['report']).parent.name)
