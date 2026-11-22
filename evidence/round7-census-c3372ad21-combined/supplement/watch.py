"""Event-trigger a new supplement when a verified matching export appears."""
from pathlib import Path
from contextlib import closing
import argparse,collections,gzip,hashlib,json,os,select,shutil,subprocess,sys,tempfile
def wait(f):
    while not f.is_file():
        target=f.parent if f.parent.is_dir() else f.parent.parent
        fd=os.open(target,os.O_RDONLY)
        try:
            with closing(select.kqueue()) as q:
                reg=select.kevent(fd,filter=select.KQ_FILTER_VNODE,flags=select.KQ_EV_ADD|select.KQ_EV_ONESHOT,fflags=select.KQ_NOTE_WRITE)
                q.control([reg],0,0)
                if not f.is_file():q.control(None,1,30)
        finally:os.close(fd)
def sha(p):
    with p.open('rb') as f:return hashlib.file_digest(f,'sha256').hexdigest()
def main():
    a=argparse.ArgumentParser()
    for k in ('even','odd','previous','chrome'):a.add_argument('--'+k,type=Path,required=True)
    a.add_argument('--mode',choices=['odd','combined'],required=True);s=a.parse_args()
    final=s.odd/('final-'+s.mode);expected={'odd':(160,15107),'combined':(320,30179)}[s.mode]
    wait(final/'SHA256SUMS')
    summ=json.loads((final/'summary.json').read_bytes());assert (summ['shardsComplete'],summ['counts']['current']['pages'])==expected
    src=Path(__file__).parent; dest=s.odd/(s.mode+'-supplement')
    temp=Path(tempfile.mkdtemp(prefix='.'+dest.name+'-',dir=s.odd))
    for name in ('extract.py','triage.py','watch.py'):shutil.copyfile(src/name,temp/name)
    subprocess.run([sys.executable,str(src/'extract.py'),'--matched',str(final),'--current-even-raw',str(s.even/'raw'),'--current-odd-raw',str(s.odd/'raw'),'--previous-raw',str(s.previous/'raw'),'--chrome-raw',str(s.chrome/'raw'),'--out',str(temp)],check=True)
    subprocess.run([sys.executable,str(src/'triage.py'),'--final',str(final),'--out',str(temp),'--even',str(s.even),'--odd',str(s.odd),'--previous',str(s.previous),'--chrome',str(s.chrome)],check=True)
    read=lambda n:json.loads((temp/n).read_bytes())
    names=read('summary.json');triage=read('loss-triage.json')
    assert triage['summary']['n']==summ['counts']['currentPrevious']['secondOnly']
    assert triage['summary']['signedStatus'].get('Pass',0)<=triage['summary']['n']
    allkeys=json.loads((final/'current-provenance.json').read_bytes())
    with gzip.open(final/'named.jsonl.gz','rb') as full,gzip.open(temp/'lp-only-vs-chrome.jsonl.gz','rb') as lp,gzip.open(temp/'chrome-only-vs-lp.jsonl.gz','rb') as chrome:
        num=collections.Counter()
        for row in map(json.loads,full):
            h,c=row[3:5];hp=h is not None and h[0]=='Pass';cp=c is not None and c[0]=='Pass'
            if hp==cp:continue
            stream=(lp if hp else chrome);assert row==json.loads(stream.readline());num['lpOnly' if hp else 'chromeOnly']+=1
        assert not lp.readline() and not chrome.readline()
    assert num['lpOnly']==names['lpOnly'] and num['chromeOnly']==names['chromeOnly']
    (temp/'directional-readback.json').write_text(json.dumps(dict(verified=True,role=s.mode,localRuntime=True,shardCount=len(allkeys),urls=expected[1],counts=num),indent=2)+'\n')
    (temp/'README.md').write_text("""# Cumulative census directional sets and loss audit

This comparison preserves exact one-based subtest occurrence identities. Both the original even and reassigned odd allocations were executed locally. The odd-only supplement covers only original odd ordinals. The combined supplement covers all 320 v2 ordinals. Signed Chrome observations are reused with their original receipts.

The two named streams record URL, canonical name, occurrence, current LP cell, signed Chrome cell and previous LP cell. Non-null cells include status and the originally reported name. Strict page change records compare to prior LP and include all three engines' raw lifecycle, harness, diagnostics, per-page counts and authenticated receipt hashes. The loss audit joins every former passing occurrence to all three raw inputs; causal labels describe recorded symptoms and do not establish which commit caused a change.

Rebuild using watch.py with --even, --odd, --previous, --chrome, and --mode. Every input source lives under its run's raw directory and requires the associated port-proof and matched comparison export. No tests are executed during reconstruction.
""")
    (temp/'SHA256SUMS').write_text(''.join(sha(p)+'  '+p.name+'\n' for p in sorted(temp.iterdir()) if p.is_file() and p.name!='SHA256SUMS'))
    if dest.exists():raise SystemExit('refusing to overwrite existing '+str(dest))
    os.rename(temp,dest);print(dest,sha(dest/'SHA256SUMS'),flush=True)
if __name__=='__main__':main()
