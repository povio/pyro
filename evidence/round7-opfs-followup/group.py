from pathlib import Path
import collections,gzip,json
w=Path(__file__).parent
scope=json.loads((w/'scope.json').read_text())
def category(name):
    if name in ('WriteParams: write missing data param','WriteParams: seek missing position param','WriteParams: truncate missing size param'):return 'current-spec-TypeError'
    if name=='write() with an invalid blob to an empty file should reject':return 'snapshot-after-deletion'
    if 'cloned via postMessage' in name or name=='Attempt to create a sync access handle.':return 'requires-handle-serialization'
    return None
per={}
for label in ('base','candidate','chrome158'):
    f=w/(label+'.json')
    if not f.is_file():continue
    run=json.loads(f.read_bytes())
    per[label]={}
    for g in ['target','guard','opfs','storage']:
        pages=[r for r in run['results'] if scope[r['url']]==g or (g=='opfs' and r['url'].startswith('fs/')) or (g=='storage' and r['url'].startswith('storage/'))]
        tests=[n for r in pages for n in r['subtests']]
        per[label][g]={'n':len(pages),'observed':len(tests),'passes':sum(n['status']=='Pass' for n in tests),
            'strict':sum(r['status']=='complete' and r.get('harnessStatus')=='OK' and bool(r['subtests']) and all(n['status']=='Pass' for n in r['subtests']) for r in pages),
            'pages':collections.Counter(r['status'] for r in pages),'harness':collections.Counter(str(r.get('harnessStatus')) for r in pages),
            'nonOK':[[r['url'],r['status'],r.get('harnessStatus'),r.get('error'),len(r['subtests'])] for r in pages if r['status']!='complete' or r.get('harnessStatus')!='OK']}
(w/'cohorts.json').write_text(json.dumps(per,indent=2)+'\n')
if (w/'named.jsonl.gz').exists():
    rows=list(map(json.loads,gzip.open(w/'named.jsonl.gz')));out=[]
    for u,n,i,b,h,c in rows:
        if scope[u]=='target' and b!='Pass' and c=='Pass':out.append([u,n,i,b,h,c,category(n)])
    (w/'residuals.json').write_text(json.dumps(out,indent=2)+'\n')
