import pathlib,hashlib,collections,sys,time,os,json,subprocess,tempfile,signal
p=pathlib.Path(__file__).resolve().parent;cache=p.parents[1];root=p/'raw';scope=sys.argv[1] if len(sys.argv)>1 else 'all';load=lambda f:json.loads(f.read_bytes());sha=lambda f:hashlib.sha256(f.read_bytes()).hexdigest()
ident=load(p/'reference-identity.json')
assert (p.parent/'version.txt').read_text().strip()=='Google Chrome 158.0.8127.37'
tasks=load(p/'plans/tasks.json')
def append(name,data):
 with (root/name).open('a') as f:f.write(json.dumps(data)+'\n')
max_new=int(os.environ.get('REFERENCE_MAX_NEW_SHARDS','0')); launched=0
for task in tasks:
 if max_new and launched>=max_new:break
 key=task['key']
 if scope=='priority' and key.startswith('ordinary'):continue
 if scope=='ordinary' and not key.startswith('ordinary'):continue
 if (p/'PAUSE_NEW').exists():break
 out=root/(key+'.json');claimed=root/(key+'.claim');failed=root/(key+'.failed.json');sel=p/'selections'/(key+'.json')
 if out.exists() or failed.exists():continue
 try:claimed.mkdir()
 except FileExistsError:continue
 try:
  if out.exists() or failed.exists():continue
  if (p/'PAUSE_NEW').exists():break
  assert sha(sel)==task['sha256']; expected=set(load(sel));assert len(expected)==task['count']
  launched+=1
  with tempfile.TemporaryDirectory(prefix='round6-158-') as profile, (root/(key+'.log')).open('x') as log:
   cmd=['bun','bench/wpt/run-no-native-retry.mjs','--kind','testharness','--wpt',str(cache/'wptci/wpt-full'),'--server','wptserve','--testdriver','--http2','--generated-globals',str(cache/'round6-inventory/MANIFEST-pinned.json'),'--manifest-inventory','--measure-reviewed-skips','--dirs',task['scope'],'--jobs','4','--tests',str(sel),'--bin','chrome158='+str(p/'launch-158'),'--out',str(out)]
   start=time.time()
   # Each start (including any restart) requires a fresh profile. In default mode
   # the launcher allocates a distinct profile directory inside this shard's
   # temporary state; a restarted process can never share the previous profile.
   state=pathlib.Path(profile);store=p/'launch-records'/key;store.mkdir(parents=True,exist_ok=True);(state/'launch-records').symlink_to(store,target_is_directory=True)
   env=dict(os.environ,WPT_CHROME_STATE_DIR=profile,WPT_CHROME_REFERENCE_MODE='wpt-stable');env.pop('WPT_CHROME_PROFILE',None)
   rc=subprocess.run(cmd,cwd=p/'runner',env=env,stdout=log,stderr=subprocess.STDOUT).returncode
  if rc or not out.is_file():
   event=dict(key=key,rc=rc,seconds=round(time.time()-start),reason='infrastructure incomplete: first observations preserved, no blind replays')
   failed.write_text(json.dumps(event,indent=2)+'\n');append('failures.jsonl',event);continue
  d=load(out);assert d['label']=='chrome158' and d['jobs']==4 and d['wptRevision']==ident['wpt'] and d['inventoryPlan']['manifestSha256']==ident['manifestSha256'] and d['inventoryPlan']['viewport']=={'width':800,'height':600,'deviceScaleFactor':1}
  rows=d['results'];assert {r['url'] for r in rows}==expected and len(rows)==len(expected)
  states=collections.Counter(r['status'] for r in rows)
  event=dict(key=key,count=len(rows),statuses=states,seconds=round(time.time()-start),restarts=d['restarts'],watchdogKills=d['watchdogKills'],receiptSHA256=sha(out))
  append('completed.jsonl',event)
  if d['restarts'] or d['watchdogKills']:append('process-events.jsonl',event)
 finally:claimed.rmdir()
