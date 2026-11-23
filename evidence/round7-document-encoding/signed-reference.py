import collections
import hashlib
import json
import sqlite3
import zlib
from pathlib import Path

work = Path(__file__).resolve().parent
cache = work.parent
source = cache/'round6-reference-158/census'
exporter = cache/'round7-local/census-c3372ad21/exporter'
db = sqlite3.connect((exporter/'cache/reference.sqlite').as_uri()+'?mode=ro', uri=True)
def meta(key):
    return json.loads(db.execute('SELECT v FROM metadata WHERE k=?',(key,)).fetchone()[0])
def digest(data):
    return hashlib.sha256(data).hexdigest()
selection = (work/'urls.json').read_bytes()
assert digest(selection)=='6844c531f0fc03e30f94c59a271e79e556efb1d61a224576f0094948b752b9fa'
urls=json.loads(selection)
assert len(urls)==len(set(urls))==14
receipt=meta('chromeReceipts')
byshard=collections.defaultdict(dict)
for url in urls:
    _,page,keys,hold,names=json.loads(zlib.decompress(db.execute('SELECT payload FROM pages WHERE url=?',(url,)).fetchone()[0]))
    assert page and hold is None and keys[1].startswith('chrome/')
    byshard[keys[1].split('/')[1]][url]=page
rows={}
dest={}
for shard,wanted in byshard.items():
    data=(source/'raw'/(shard+'.json')).read_bytes()
    proof=receipt[shard]
    assert digest(data)==proof['rawSHA256']
    run=json.loads(data)
    assert run['bin']==str(source/'launch-158') and run['wptRevision']=='a421f2fea5eb7f91e0db399e44b0fb0779ee1603'
    assert run['jobs']==4 and run['server']=='wptserve' and run['testdriver'] and run['http2']
    assert proof['restarts']==run['restarts']==0 and proof['watchdogKills']==run['watchdogKills']==0
    for r in run['results']:
        url=r['url']
        if url not in wanted:continue
        page=wanted[url]
        assert [r['status'],r['harnessStatus']]==page[:2]
        assert len(r['subtests'])==page[4] and sum(t['status']=='Pass' for t in r['subtests'])==page[3]
        assert url not in rows
        rows[url]=r
    dest[shard]=dict(**proof,selectedURLs=sorted(wanted))
assert set(rows)==set(urls)
def save(name,data):
    p=work/name
    t=p.with_suffix('.tmp')
    t.write_text(json.dumps(data,separators=(',',':'),ensure_ascii=True)+'\n')
    t.replace(p)
save('chrome158.json',dict(version='158.0.8127.37',wptRevision=run['wptRevision'],jobs=4,source='subset of frozen signed Chrome shard receipts',plan=urls,restarts=0,watchdogKills=0,results=[rows[u] for u in urls]))
save('chrome158-map.json',dict(selectionSHA256=digest(selection),referenceCommit='01f49f43da24a20ecb20d99c967fc5de54272120',referenceIdentity=meta('identity'),chromeSHA256=digest((work/'chrome158.json').read_bytes()),shards=dest))
print('source shards',len(dest),'status',collections.Counter(r['status'] for r in rows.values()),'names',sum(len(r['subtests']) for r in rows.values()),'passed',sum(t['status']=='Pass' for r in rows.values() for t in r['subtests']))
