from pathlib import Path
import collections,gzip,json,statistics
q=Path(__file__).resolve().parent
def quant(vals,x):
 vals=sorted(vals);p=(len(vals)-1)*x;l=int(p);return vals[l]+(vals[min(l+1,len(vals)-1)]-vals[l])*(p-l)
def stats(vals):return {'median':statistics.median(vals),'p10':quant(vals,.1),'p90':quant(vals,.9),'samples':vals}
def names(r):
 c=collections.Counter();idx={}
 for v in r['subtests']:n=v['name'];c[n]+=1;idx[n,c[n]]=v['status']
 return idx
summary={}
for suite in ['probe','wpt']:
 labs=['previous','agent','chrome158']
 docs={k:json.load(open(q/f'{suite}-{k}.json')) for k in labs}; rows={k:{r['url']:r for r in v['results']} for k,v in docs.items()}
 assert len({tuple(sorted(v)) for v in rows.values()})==1
 totals=collections.Counter();pages={};delta=[]
 for u in sorted(rows['agent']):
  data={k:names(v[u])for k,v in rows.items()};c=collections.Counter()
  for k,d in data.items():
   row=rows[k][u];c[k+'Pass']=sum(s=='Pass' for s in d.values());c[k+'Reported']=len(d);c[k+'Full']=int(row['status']=='complete' and row.get('harnessStatus','OK')=='OK' and bool(d) and c[k+'Pass']==len(d));c[k+'Page_'+row['status']]+=1
  for key in sorted(set().union(*data.values())):
   prev,agent,chrome=[d.get(key) for d in data.values()];delta.append([u,*key,prev,agent,chrome]);c['gain']+=prev!='Pass' and agent=='Pass';c['loss']+=prev=='Pass' and agent!='Pass';c['missing']+=prev is not None and agent is None;c['newNames']+=prev is None and agent is not None;c['chromeOnly']+=chrome=='Pass' and agent!='Pass';c['agentOnly']+=agent=='Pass' and chrome!='Pass'
  pages[u]=c;totals.update(c)
 summary[suite]={'pages':len(pages),'scores':totals,'perURL':pages,'restarts':{k:v['restarts'] for k,v in docs.items()},'watchdogKills':{k:v['watchdogKills'] for k,v in docs.items()}}
 with gzip.open(q/f'{suite}-named.jsonl.gz','wt') as out:
  for r in delta:print(json.dumps(r),file=out)
raw=json.load(open(q/'application-results.json'));assert len(raw)==15
perf={}
for label in sorted({r['lab'] for r in raw}):
 process=[r for r in raw if r['lab']==label];assert len(process)==5
 out={'startupMs':stats([r['startupMs'] for r in process]),'load1':stats([r['load'][0] for r in process])}
 for stage in process[0]['stages']:
  v=[r['stages'][stage] for r in process];out[stage+'UsPerBlock']=stats([statistics.median(s['samples'])*1000/s['batch'] for s in v])
 out['callbackCountsPerProcess']=[r['callbacks'] for r in process];perf[label]=out
summary['application']=perf
(q/'summary.json').write_text(json.dumps(summary,indent=2)+'\n')
print({s:dict(summary[s]['scores']) for s in ['probe','wpt']})
for label,ms in perf.items():print(label,{k:round(v['median'],2) for k,v in ms.items() if k.endswith('UsPerBlock')})
