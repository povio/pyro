from pathlib import Path
import json,statistics
q=Path(__file__).resolve().parent
rows=json.load(open(q/'cpu-application-results.json'));assert len(rows)==15
def quant(l,x):
 v=sorted(l);pos=(len(v)-1)*x;lo=int(pos);return v[lo]+(v[min(lo+1,len(v)-1)]-v[lo])*(pos-lo)
def summary(l):return dict(median=statistics.median(l),p10=quant(l,.1),p90=quant(l,.9),perProcess=l)
out={}
for e in ['previous','agent','chrome158']:
 group=sorted([r for r in rows if r['lab']==e],key=lambda r:r['round']);assert len(group)==5
 data={}
 for phase in group[0]['measured']:
  values=[];walls=[];changed=0
  for r in group:
   a=r['measured'][phase];assert len(a)==3
   costs=[v['cpuUsPerBlock'] for v in a if v['stable']];changed+=len(a)-len(costs)
   if costs:values.append(statistics.median(costs))
   walls.append(statistics.median(v['wallUsPerBlock'] for v in a))
  data[phase]={'cpuUsPerBlock':summary(values),'wallUsPerBlock':summary(walls),'unstableProcessSamplesOmittedFromCpu':changed}
 out[e]=dict(stages=data,load1=summary([r['load'][0] for r in group]))
(q/'cpu-summary.json').write_text(json.dumps(out,indent=2)+'\n')
for e,d in out.items():
 print(e, 'load',d['load1']['median'],[(k,round(v['cpuUsPerBlock']['median'],1),round(v['cpuUsPerBlock']['p10'],1),round(v['cpuUsPerBlock']['p90'],1),v['unstableProcessSamplesOmittedFromCpu']) for k,v in d['stages'].items()])
