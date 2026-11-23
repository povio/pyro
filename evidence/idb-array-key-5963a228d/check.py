import hashlib, mmap, pathlib, sys, subprocess, json
p=pathlib.Path(__file__).resolve().parent
arm,binary=sys.argv[1:]
ref={'base':'11597f2e0','candidate':'5963a228d'}[arm]
def git(*args): return subprocess.check_output(['git','-C',str(p/'src'),*args])
def script(rev): return git('show',rev+':crates/pyro-webapi/src/idb.js')
with open(binary,'rb') as f, mmap.mmap(f.fileno(),0,access=mmap.ACCESS_READ) as b:
    offset=b.find(script(ref))
    assert offset>=0 and b.find(script('5963a228d' if arm=='base' else '11597f2e0'))<0
    print(json.dumps(dict(commit=git('rev-parse',ref).decode().strip(),tree=git('rev-parse',ref+'^{tree}').decode().strip(),binary=binary,sha256=hashlib.sha256(b).hexdigest(),idbOffset=offset,idbSha256=hashlib.sha256(script(ref)).hexdigest())))
