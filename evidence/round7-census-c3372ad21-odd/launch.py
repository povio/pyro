"""One coordinator per machine; one independent process per four-page shard."""
import collections,fcntl,hashlib,json,os,pathlib,select,subprocess,sys,tempfile,time
from datetime import datetime,timezone
P=pathlib.Path(__file__).resolve().parent
C=P.parents[1]; REF=C/'round6-reference-158/census'; RAW=P/'raw'
sys.path.insert(0,str(REF))
from port_receipt import snapshot_config
UTC=lambda:datetime.now(timezone.utc).isoformat()
def load(p):return json.loads(p.read_bytes())
def sha(p):
    with p.open('rb') as f:return hashlib.file_digest(f,'sha256').hexdigest()
def atomic(path,obj):
    with tempfile.NamedTemporaryFile(mode='w',dir=path.parent,delete=False) as f:
        json.dump(obj,f,separators=(',',':'));f.write('\n');f.flush();os.fsync(f.fileno())
    os.replace(f.name,path)
def append(name,row):
    with (RAW/name).open('a') as f:
        fcntl.flock(f,fcntl.LOCK_EX);f.write(json.dumps(row,separators=(',',':'))+'\n');f.flush();os.fsync(f.fileno())
def ledger():
    p=RAW/'completed.jsonl'
    if not p.exists():return {}
    with p.open() as f:
        fcntl.flock(f,fcntl.LOCK_SH);rows=[json.loads(line) for line in f if line.strip()]
    assert len({r['key'] for r in rows})==len(rows)
    return {r['key']:r for r in rows}
def init():
    meta=load(P/'machine-provenance.json'); plan_dir=P/meta['planDirectory']; plan=load(plan_dir/'tasks.json')
    assert sha(plan_dir/'tasks.json')==meta['effectiveManifestSHA256']
    assert sha(REF/'runner/bench/wpt/run-no-native-retry.mjs')==meta['runnerSHA256']
    return meta,plan
def run_one(index):
    meta,plan=init();t=plan[index];k=t['key'];assert t['index']==index and index%2==1
    sel=P/meta['planDirectory']/'selections'/(k+'.json');urls=load(sel)
    assert sha(sel)==t['sha256'] and len(urls)==len(set(urls))==t['count']
    with (RAW/(k+'.lock')).open('a') as lock:
        try:fcntl.flock(lock,fcntl.LOCK_EX|fcntl.LOCK_NB)
        except BlockingIOError:return
        if k in ledger() or (RAW/(k+'.unresolved')).exists(): return
        result=RAW/(k+'.json');log=RAW/(k+'.log');trace=RAW/(k+'.trace')
        # Interrupted worker with a completed raw file can finalize its receipt.
        # A prior attempt that started navigation must be assessed from its
        # preserved trace/checkpoint; never silently replay an affected URL.
        if not result.exists():
            if log.exists():
                attempted=trace.exists() and bool(trace.read_bytes().strip())
                if attempted or (RAW/(k+'.retry1.log')).exists():
                    atomic(RAW/(k+'.unresolved'),{'reason':'previous run incomplete','key':k,'startedURLs':attempted});return
                log.rename(RAW/(k+'.retry1.log'))
                for path in [trace,RAW/(k+'.json.partial'),RAW/(k+'.start.json')]:
                    if path.exists():path.rename(RAW/(path.name+'.first'))
            args=['bun','bench/wpt/run-no-native-retry.mjs','--kind','testharness','--wpt',str(C/'wptci/wpt-full'),'--server','wptserve','--testdriver','--http2','--generated-globals',str(C/'round6-inventory/MANIFEST-pinned.json'),'--manifest-inventory','--measure-reviewed-skips','--dirs','non-css','--jobs','4','--tests',str(sel),'--bin','lp='+meta['bin'],'--out',str(result)]
            started=time.time()
            with log.open('x') as out:
                proc=subprocess.Popen(args,cwd=REF/'runner',env=dict(os.environ,WPT_TRACE=str(trace)),stdout=out,stderr=subprocess.STDOUT)
                atomic(RAW/(k+'.start.json'),{'key':k,'ordinal':index,'actualExecutor':'local','originalOwner':'studio','pid':os.getpid(),'runnerPID':proc.pid,'start':started,'utc':UTC(),'selectorSHA256':t['sha256'],'artifactSHA256':meta['binarySHA256']})
                rc=proc.wait()
            if rc or not result.exists():
                append('failures.jsonl',dict(key=k,utc=UTC(),returncode=rc))
                if trace.exists() and trace.stat().st_size:
                    atomic(RAW/(k+'.unresolved'),dict(key=k,reason='runner failed after navigation began',returncode=rc))
                return
        r=load(result);rs=r['results'];start=load(RAW/(k+'.start.json'))
        assert r['bin']==meta['bin'] and r['jobs']==4 and r['server']=='wptserve' and r['testdriver'] and r['http2'] and not r['reuseContext']
        assert r['wptRevision']==meta['wpt'] and r['inventoryPlan']['manifestSha256']==meta['manifestSHA256']
        assert r['inventoryPlan']['viewport']=={'width':800,'height':600,'deviceScaleFactor':1}
        assert set(r['plan'])==set(urls)=={v['url'] for v in rs} and len(rs)==len(urls)
        _,proof=snapshot_config(log,P/'verified-ports'/(k+'.json'))
        status=collections.Counter(v['status'] for v in rs)
        withheld=[v['url'] for v in rs if v['status'] in ('hang','crash','panic') or 'withheld without replay' in (v.get('error') or '')]
        if withheld or r['restarts'] or r['watchdogKills']:
            append('native-events.jsonl',dict(key=k,explicit=withheld,unassignedNativeExitPossible=bool(r['restarts'] or r['watchdogKills']),restarts=r['restarts'],watchdogKills=r['watchdogKills'],retried=False))
        append('completed.jsonl',dict(key=k,index=index,owner=t['owner'],actualExecutor='local',count=len(rs),selectionSHA256=t['sha256'],receiptSHA256=sha(result),logSHA256=sha(log),configSHA256=proof['sha256'],statuses=status,wallMs=r['wallMs'],restarts=r['restarts'],watchdogKills=r['watchdogKills'],seconds=round(time.time()-start['start'],2),utc=UTC()))
def active_files(tasks):
    found=set()
    for t in tasks:
        p=RAW/(t['key']+'.lock')
        if not p.exists():continue
        with p.open('r') as f:
            try:fcntl.flock(f,fcntl.LOCK_EX|fcntl.LOCK_NB)
            except BlockingIOError:found.add(t['key'])
    return found
def supervisor():
    meta,plan=init();tasks=[t for t in plan if t['index']%2==1]
    assert sha(pathlib.Path(meta['bin']))==meta['binarySHA256'] and len(tasks)==160
    grant=load(P/'allocation.json')
    assert grant['local']=='odd' and grant['manifestSHA256']==meta['effectiveManifestSHA256']
    assert all(t['owner']=='studio' for t in tasks)
    with (P/'supervisor.lock').open('a') as lock:
        fcntl.flock(lock,fcntl.LOCK_EX|fcntl.LOCK_NB)
        children={}
        changes=select.kqueue()
        while True:
            for k,proc in list(children.items()):
                if proc.poll() is not None:
                    if proc.returncode:atomic(RAW/(k+'.unresolved'),dict(key=k,reason='worker exception; inspect supervisor log',returncode=proc.returncode))
                    del children[k]
            done=ledger();active=active_files(tasks)|set(children)
            pending=[t for t in tasks if t['key'] not in done and t['key'] not in active and not (RAW/(t['key']+'.unresolved')).exists()]
            if not pending and not active:
                return 0 if all(t['key'] in done for t in tasks) else 2
            limit=int((P/'slots').read_text());assert 0<=limit<=16
            for t in pending[:max(0,limit-len(active))]:
                proc=subprocess.Popen([sys.executable,__file__,'--one',str(t['index'])],start_new_session=True)
                children[t['key']]=proc
                try:
                    event=select.kevent(proc.pid,filter=select.KQ_FILTER_PROC,
                        flags=select.KQ_EV_ADD|select.KQ_EV_ONESHOT,fflags=select.KQ_NOTE_EXIT)
                    changes.control([event],0,0)
                except ProcessLookupError:pass
            # Completion wakes admission immediately. Timeout also notices
            # externally updated capacity and independently recovered workers.
            changes.control(None,16,5)
if __name__=='__main__':
    if len(sys.argv)==3 and sys.argv[1]=='--one':run_one(int(sys.argv[2]))
    else:sys.exit(supervisor())
