"""Keep the independent GPU-disabled two-job Chrome 158 trial out of matched counts."""
from pathlib import Path
import gzip,json,hashlib,sys
p=Path(sys.argv[1]);d=Path(sys.argv[2])
rawfile=d/'target-chrome158.json';sel=json.loads((d/'target-urls.json').read_bytes())
raw=json.loads(rawfile.read_bytes())
assert raw['jobs']==2 and raw['wptRevision']=='a421f2fea5eb7f91e0db399e44b0fb0779ee1603'
assert sorted(sel)==sorted(r['url'] for r in raw['results'])
assert '--disable-gpu' in (d/'chrome158').read_text() and (d/'chrome-version.txt').read_text().strip()=='Google Chrome 158.0.8127.37'
stored=[json.loads(l) for l in gzip.open(p/'pages.jsonl.gz','rt') if '"wasm-storage-supplement"' in l]
match={row[0]:row for row in stored}
assert len(match)==5 and set(sel)<=set(match)
rows=[]
for r in raw['results']:
 u=r['url'];row=match[u];assert (r['source'],r['blob'])==tuple(row[4][:2])
 rows.append(dict(url=u,source=r['source'],blob=r['blob'],status=r['status'],harnessStatus=r['harnessStatus'],subtests=r['subtests'],lpBasePage=row[5],lpHeadPage=row[6]))
s=lambda f:hashlib.sha256(f.read_bytes()).hexdigest()
(p/'storage-alternate-reference.json').write_text(json.dumps(dict(note='Two-job Google Chrome stable 158.0.8127.37 trial, --disable-gpu and separate disposable profile. Kept out of the primary four-job signed WPT profile scores.',rawSHA256=s(rawfile),launcherSHA256=s(d/'chrome158'),logSHA256=s(d/'target-chrome158.log'),rows=rows),indent=1)+'\n')
