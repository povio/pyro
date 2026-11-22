"""Pack source receipts losslessly, preserving their original independent hashes."""
from pathlib import Path
import collections,datetime,gzip,hashlib,json,shutil,sys
p=Path(__file__).resolve().parent
out=Path(sys.argv[1]).resolve();out.mkdir(exist_ok=True,parents=True)
def sha(f):
    with f.open('rb') as data:return hashlib.file_digest(data,'sha256').hexdigest()
def copy(src,dst):
    dst.parent.mkdir(exist_ok=True,parents=True);shutil.copyfile(src,dst)
ledgers=[json.loads(l) for l in (p/'raw/completed.jsonl').read_text().splitlines()]
allowed=set(json.loads(Path(sys.argv[2]).read_bytes())) if len(sys.argv)>2 else {r['key'] for r in ledgers}
raw=out/'raw';raw.mkdir(exist_ok=True)
for row in ledgers:
    k=row['key']
    if k not in allowed:continue
    src=p/'raw'/(k+'.json');dest=raw/(k+'.json.gz');assert sha(src)==row['receiptSHA256']
    with src.open('rb') as stream,dest.open('wb') as actual,gzip.GzipFile(filename='',fileobj=actual,mode='wb',compresslevel=4,mtime=0) as zipped:
        shutil.copyfileobj(stream,zipped,1024*1024)
    with gzip.open(dest,'rb') as check:assert hashlib.file_digest(check,'sha256').hexdigest()==row['receiptSHA256']
    for suffix in ('.log','.start.json'):
        copy(p/'raw'/(k+suffix),raw/(k+suffix))
    copy(p/'verified-ports'/(k+'.json'),out/'verified-ports'/(k+'.json'))
(raw/'completed.jsonl').write_text(''.join(json.dumps(r,separators=(',',':'))+'\n' for r in ledgers if r['key'] in allowed))
for n in ('failures.jsonl','native-events.jsonl'):
    if (p/'raw'/n).exists():copy(p/'raw'/n,raw/n)
copy(p/'machine-provenance.json',out/'machine-provenance.json')
