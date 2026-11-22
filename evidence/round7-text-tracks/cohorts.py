from pathlib import Path
import collections,gzip,json
w=Path(__file__).resolve().parent
groups=json.loads((w/'groups.json').read_text())
data=[json.loads(l) for l in gzip.open(w/'named.jsonl.gz','rt')]
pages=json.loads((w/'pages.json').read_text());groups['fileParser']=[u for u in groups['target'] if '/file-parsing/' in u]
groups['cueParser']=[u for u in groups['target'] if '/cue-text-parsing/' in u]
groups['webvttAPI']=[u for u in groups['target'] if u.startswith('webvtt/api/')]
groups['mediaTrackObjects']=[u for u in groups['target'] if u.startswith('html/')]
groups['unique']=groups['target']+groups['guards']
out={}
for group,urls in groups.items():
 rows=[r for r in data if r[0] in urls];pg=[r for r in pages if r[0] in urls]
 d=out[group]={'urls':len(urls),'unionOccurrences':len(rows)}
 for i,name in enumerate(['parent','candidate','signed'],3):
  d[name]={'pass':sum(r[i]=='Pass' for r in rows),'missing':sum(r[i] is None for r in rows),'strict':sum(r[i-2][2] for r in pg),'pages':dict(collections.Counter(r[i-2][0] for r in pg))}
 d.update(recoveredObserved=sum(b is not None and b!='Pass' and c=='Pass' for _,_,_,b,c,s in rows),newlyReportedPass=sum(b is None and c=='Pass' for _,_,_,b,c,s in rows),newlyReportedOther=sum(b is None and c not in (None,'Pass') for _,_,_,b,c,s in rows),losses=sum(b=='Pass' and c!='Pass' for _,_,_,b,c,s in rows),signedOnly=sum(s=='Pass' and c!='Pass' for _,_,_,b,c,s in rows))
(w/'cohort-scores.json').write_text(json.dumps(out,indent=2)+'\n')
