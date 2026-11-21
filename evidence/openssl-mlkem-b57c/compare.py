"""Read raw observations and preserve URL/name/occurrence and strict page pass sets."""
from pathlib import Path
import collections,gzip,json
r=Path(__file__).resolve().parent/'results'
def read(label):
 data=json.loads((r/(label+'.json')).read_text())['results'];names={};pages={};strict=[]
 for t in data:
  url=t['url'];pages[url]=[t['status'],t.get('harnessStatus')];seen=collections.Counter()
  for s in t['subtests']:
   name=s['name'];seen[name]+=1;names[url,name,seen[name]]=s['status']
  if pages[url]==['complete','OK'] and t['subtests'] and all(s['status']=='Pass' for s in t['subtests']):strict.append(url)
 return {'pages':pages,'strict':sorted(strict),'pass':sorted(k for k,v in names.items() if v=='Pass'),'names':sorted(names),'statuses':dict(collections.Counter(names.values()))}
observations={k:read(k) for k in ['base','reviewed','chrome','default-topic','guard-base','guard-reviewed','probes-base','probes-reviewed']}
def delta(a,b):
 a,b=observations[a],observations[b]
 assert a['pages'].keys()==b['pages'].keys()
 return {'gained':sorted(set(b['pass'])-set(a['pass'])),'lost':sorted(set(a['pass'])-set(b['pass'])),'missing':sorted(set(a['names'])-set(b['names'])),'added':sorted(set(b['names'])-set(a['names'])),'strictGained':sorted(set(b['strict'])-set(a['strict'])),'strictLost':sorted(set(a['strict'])-set(b['strict']))}
report={'observations':observations,'officialDelta':delta('base','reviewed'),'guardDelta':delta('guard-base','guard-reviewed'),'extraDelta':delta('probes-base','probes-reviewed')}
(r/'named-and-page-sets.json.gz').write_bytes(gzip.compress(json.dumps(report,separators=(',',':')).encode(),mtime=0))
