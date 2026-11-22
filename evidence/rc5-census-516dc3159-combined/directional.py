"""Independently derive strict-page and named asymmetric pass sets from the joined cells."""
from pathlib import Path
from collections import Counter,defaultdict
from contextlib import ExitStack
import gzip,hashlib,json,sys
def extract(src,out):
 out.mkdir(exist_ok=True,parents=True)
 summary=json.loads((src/'summary.json').read_bytes());count=Counter()
 with gzip.open(src/'coverage.jsonl.gz','rt') as f:env={r[0]:r[4] for r in map(json.loads,f)}
 byarea=defaultdict(Counter);byenv=defaultdict(Counter)
 with ExitStack() as stack:
  sinks={}
  for key in ('lp-only-names','chrome-only-names','lp-only-pages','chrome-only-pages'):
   raw=stack.enter_context((out/(key+'.jsonl.gz')).open('wb')); sinks[key]=stack.enter_context(gzip.GzipFile(filename='',mode='wb',fileobj=raw,compresslevel=3,mtime=0))
  for group in ('named','pages'):
   isname=group=='named'
   with gzip.open(src/(group+'.jsonl.gz'),'rb') as f:
    for row in map(json.loads,f):
     lp,chrome=(row[3:5] if isname else row[1:3])
     def passed(x):return x is not None and (x[0]=='Pass' if isname else x[2])
     lp,chrome=passed(lp),passed(chrome);label='names' if isname else 'pages'
     area=row[0].lstrip('/').split('/',1)[0]
     scopes=(count,byarea[area],byenv[env[row[0]]])
     for tally in scopes:
      tally['lp-'+label]+=lp;tally['chrome-'+label]+=chrome
     if lp!=chrome:
      key=('lp' if lp else 'chrome')+'-only-'+label
      for tally in scopes:tally[key]+=1
      sinks[key].write(json.dumps(row,separators=(',',':')).encode()+b'\n')
 for s,k,l in [('lp','current','first'),('chrome','chrome','second')]:
  assert count[s+'-names']==summary['counts'][k]['passingNames'] and count[s+'-pages']==summary['counts'][k]['strictPages']
  assert count[s+'-only-names']==summary['counts']['currentChrome'][l+'Only']
  assert count[s+'-only-pages']==summary['counts']['currentChrome'][l+'OnlyPage']
 (out/'by-area.json').write_text(json.dumps(byarea,indent=2,sort_keys=True)+'\n')
 (out/'by-environment.json').write_text(json.dumps(byenv,indent=2,sort_keys=True)+'\n')
 (out/'readback.json').write_text(json.dumps(count,indent=2,sort_keys=True)+'\n')
if __name__=='__main__':extract(Path(sys.argv[1]),Path(sys.argv[2]))
