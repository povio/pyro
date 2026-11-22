from pathlib import Path
from collections import Counter
import gzip,json,sys
w=Path(__file__).parent
labels=['base','candidate','chrome158']
def publish(label, data):
    (w/(label+'.json')).write_text(json.dumps(data,ensure_ascii=False,indent=2)+'\n')
def classify(url):return 'api' if url.startswith('cookiestore/') else 'legacy'
per={}
for label in labels:
    if not (w/(label+'.json')).exists():continue
    run=json.loads((w/(label+'.json')).read_text());p=run['results'];per[label]={}
    for group in ['api','legacy']:
        pages=[r for r in p if classify(r['url'])==group]
        per[label][group]={'pages':len(pages),'observedNames':sum(len(r['subtests']) for r in pages),
            'namedPasses':sum(t['status']=='Pass' for r in pages for t in r['subtests']),
            'strict':sum(r['status']=='complete' and r['harnessStatus']=='OK' and bool(r['subtests']) and all(t['status']=='Pass' for t in r['subtests']) for r in pages),
            'pageOutcome':dict(Counter(r['status'] for r in pages)),
            'harnessOutcome':dict(Counter(str(r['harnessStatus']) for r in pages)),
            'nonOK':[[r['url'],r['status'],r['harnessStatus'],r.get('error'),len(r['subtests'])] for r in pages if r['status']!='complete' or r['harnessStatus']!='OK']}
publish('scope-scores',per)
if len(per)!=3:sys.exit()
rows=list(map(json.loads,gzip.open(w/'named.jsonl.gz')));pages=json.loads((w/'pages.json').read_text())
for group in ['api','legacy']:
    selected=[r for r in rows if classify(r[0])==group];counts=Counter()
    for u,n,i,b,h,c in selected:
        if b!='Pass' and h=='Pass':counts['gained']+=1;counts['newPass' if b is None else 'matchedGain']+=1;counts['gainChrome'+str(c)]+=1
        if b=='Pass' and h!='Pass':counts['lost']+=1
        if h is not None and b is None:counts['newName']+=1
        if h is None and b is not None:counts['missingName']+=1
    with gzip.open(w/(group+'-names.jsonl.gz'),'wt') as out:
        for row in selected:out.write(json.dumps(row,ensure_ascii=False,separators=(',',':'))+'\n')
    publish(group+'-pages',[r for r in pages if classify(r[0])==group]);per['candidate'][group]['delta']=dict(counts)
publish('scope-scores',per)
