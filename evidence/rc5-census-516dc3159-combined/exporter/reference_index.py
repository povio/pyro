"""Build a random-access index of the immutable prior public census once."""
import argparse
import gzip
import hashlib
import itertools
import json
import os
import sqlite3
import tempfile
import zlib
from pathlib import Path

HEAD = '516dc3159ff4602bd3a11e98607c262c0ffc65c4'
TREE = '39850661325fc24f3cd9e9f591d14df55a5fa359'
PREVIOUS = '1f1be032436bfeee9f48cdb5acbc0f71d21d5af9'
WPT = 'a421f2fea5eb7f91e0db399e44b0fb0779ee1603'
MANIFEST = '34a67512a84d43630f7adbecebd4378df5f410446591015cc6a20a3034e02819'
JSC = '3deb71f8e7d14daf174e35340a4ff0c1900b0e9f576503adeb5facbb1201f029'
RUNNER = 'b82c52329f881a5193e21ceb22634517c4cb776bd39bb080692c24eb21ac594a'
REFERENCE = '01f49f43da24a20ecb20d99c967fc5de54272120'
PINNED = {
    'named.jsonl.gz':'9197a28dd9d8e52e210c5d7373f1f89642f58a92f398c4521e2d7ce70e5c959c',
    'pages.jsonl.gz':'5e0ae28ea74e599ab87edd2fa851c5a1b9b6d65f137d4a50dafe61b609bdc9ec',
    'raw-provenance.json':'d7b1e968bdaecfd0b81638593543e937c1d2e12865d24b73cc07fab19ab9ff0e',
    'frozen-chrome-provenance.json':'a261cb279d7621e49a62386f62c77f4a18283edee8d309ea90f9d5ef4e9b26d5',
    'reference-identity.json':'ca6777e00cc8f47457f1ce8f595de1f900476434f2a93ca3bfa6308116da0f38',
    'summary.json':'2070ffaaf939475dd4817cc8baef6545bae9b3fefdd52e4b7b1bdac7b3ad4319',
}

def require(condition, message):
    if not condition:
        raise ValueError(message)

def compact(obj):
    return json.dumps(obj, separators=(',',':'), ensure_ascii=True).encode('ascii')

def sha(path):
    with Path(path).open('rb') as stream:
        return hashlib.file_digest(stream,'sha256').hexdigest()

def read(path):
    return json.loads(Path(path).read_bytes())

def readlines(path):
    with gzip.open(path, 'rb') as stream:
        yield from map(json.loads,stream)

def prepare(prior, db):
    if db.exists():
        with sqlite3.connect(db.as_uri()+'?mode=ro',uri=True) as conn:
            manifest=json.loads(conn.execute("SELECT v FROM metadata WHERE k='pinned'").fetchone()[0])
            require(manifest==PINNED,'reference index provenance mismatch')
        return
    for name,digest in PINNED.items():
        require(sha(prior/name)==digest,f'wrong frozen reference artifact: {name}')
    summary=read(prior/'summary.json')
    require((summary['head'],summary['wpt'],summary['manifestSHA256'],summary['chrome'])==
            (PREVIOUS,WPT,MANIFEST,'158.0.8127.37'),'invalid frozen comparison identity')
    db.parent.mkdir(parents=True,exist_ok=True)
    fd,tmp=tempfile.mkstemp(dir=db.parent,prefix='reference-',suffix='.sqlite');os.close(fd)
    try:
        conn=sqlite3.connect(tmp)
        with conn:
            conn.execute('CREATE TABLE metadata(k TEXT PRIMARY KEY, v TEXT)')
            conn.execute('CREATE TABLE pages(url TEXT PRIMARY KEY, payload BLOB NOT NULL)')
            grouped=itertools.groupby(readlines(prior/'named.jsonl.gz'),key=lambda row:row[0])
            group=next(grouped,None)
            n=0
            for url,previous,chrome,_,keys,hold in readlines(prior/'pages.jsonl.gz'):
                names=[]
                if group is not None and group[0]==url:
                    names=[[name,occ,old,stable] for _,name,occ,old,stable,_ in group[1] if old is not None or stable is not None]
                    group=next(grouped,None)
                for i,page in ((2,previous),(3,chrome)):
                    if page is not None:
                        require(page[4]==sum(row[i] is not None for row in names)
                              and page[3]==sum(row[i] is not None and row[i][0]=='Pass' for row in names),
                              f'prior name count mismatch for {url}')
                blob=zlib.compress(compact([previous,chrome,keys[:2],hold,names]),1)
                conn.execute('INSERT INTO pages VALUES (?,?)',(url,blob))
                n+=1
            require(group is None and n==31827,'frozen pages and names out of sync')
            for k,v in [('pinned',PINNED),('identity',read(prior/'reference-identity.json')),
                        ('previousReceipts',read(prior/'raw-provenance.json')),
                        ('chromeReceipts',read(prior/'frozen-chrome-provenance.json'))]:
                conn.execute('INSERT INTO metadata VALUES (?,?)',(k,json.dumps(v)))
        conn.close()
        os.replace(tmp,db)
    finally:
        Path(tmp).unlink(missing_ok=True)

if __name__=='__main__':
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('prior',type=Path)
    parser.add_argument('db',type=Path)
    args=parser.parse_args()
    prepare(args.prior.resolve(),args.db.resolve())
