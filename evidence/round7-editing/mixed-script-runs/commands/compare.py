import collections,json,pathlib,gzip
root=pathlib.Path(__file__).parent
labels=('base','head','chrome')
def load(label):
    pages={}
    for r in json.loads((root/f'{label}.json').read_text())['results']:
        seen=collections.Counter()
        named={}
        for t in r['subtests']:
            name=t['name']
            seen[name]+=1
            named[(name,seen[name])]=t['status']
        pages[r['url']]=named
    return pages
base,head,chrome=map(load,labels)
assert base.keys()==head.keys()==chrome.keys()
rows=[]
losses=[]
for url in sorted(base):
    a,b,c=(p[url] for p in (base,head,chrome))
    keys=a.keys()|b.keys()|c.keys()
    gains=sum(a.get(k)!='Pass' and b.get(k)=='Pass' for k in keys)
    loss=[k for k in keys if a.get(k)=='Pass' and b.get(k)!='Pass']
    losses.extend((url,k) for k in loss)
    print(url,*(sum(i=='Pass' for i in v.values()) for v in [a,b,c]),'gain',gains,'loss',len(loss),'membership delta',len(a.keys()^b.keys()))
    rows.extend([url,*k,a.get(k),b.get(k),c.get(k)] for k in sorted(keys))
(root/'named.jsonl.gz').write_bytes(gzip.compress(('\n'.join(json.dumps(r) for r in rows)+'\n').encode(),mtime=0))
print('Total',len(rows),'losses',len(losses),'gains',sum(a!='Pass' and b=='Pass' for *_,a,b,c in rows))
for case in losses[:25]:print('Loss',case)
