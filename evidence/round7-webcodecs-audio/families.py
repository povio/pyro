from pathlib import Path
import collections,gzip,json
w=Path(__file__).resolve().parent
members=json.loads((w/'groups.json').read_text())
data=[json.loads(l) for l in gzip.open(w/'named.jsonl.gz','rt')]
pages=json.loads((w/'pages.json').read_text())
groups=dict(members)
groups['unique']=sorted(set(sum(members.values(),[])))
for u in members['target']:
 f=u.split('/')[1]
 fam='IDL' if 'idlharness' in f.lower() else 'decoder' if ('audiodecoder' in f.lower().replace('-','') or 'opus-multichannel' in f) else 'data-chunk-transport'
 groups.setdefault(fam,[]).append(u)
out={}
for group,urls in groups.items():
 selected=[r for r in data if r[0] in urls]
 stats=dict(urls=len(set(urls)),allUnionOccurrences=len(selected))
 for i,name in enumerate(['parent','candidate','signed'],3):
  stats[name]=dict(passNames=sum(r[i]=='Pass' for r in selected),missingNames=sum(r[i] is None for r in selected),strictPages=sum(r[i-2][2] for r in pages if r[0] in urls),pageOutcomes=dict(collections.Counter(r[i-2][0] for r in pages if r[0] in urls)))
 stats.update(gains=sum(b!='Pass' and c=='Pass' for _,_,_,b,c,s in selected),losses=sum(b=='Pass' and c!='Pass' for _,_,_,b,c,s in selected),signedOnly=sum(c!='Pass' and s=='Pass' for _,_,_,b,c,s in selected),lpOnly=sum(c=='Pass' and s!='Pass' for _,_,_,b,c,s in selected))
 out[group]=stats
(w/'cohort-scores.json').write_text(json.dumps(out,indent=2)+'\n')
