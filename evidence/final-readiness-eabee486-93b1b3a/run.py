import datetime, hashlib, json, os, pathlib, re, subprocess, sys, time
out=pathlib.Path('/private/tmp/lp-final-readiness-93b1b3a')
wt=pathlib.Path('/private/tmp/lp-tos-f9f-batch16-fx')
binary=pathlib.Path('/private/tmp/lp-final-readiness-93b1b3a/lightningpanda-A')
assert hashlib.sha256(binary.read_bytes()).hexdigest() == '66e7448e07666176e9650ba714cba04ecc570c65f6e6139ad5bd759f5b49ad88'
proof=json.loads((out/'binary-provenance.json').read_text())
for item in proof['binaries'].values():
 assert hashlib.sha256(pathlib.Path(item['path']).read_bytes()).hexdigest()==item['sha256']
slug='lpfinalready-'+datetime.datetime.now(datetime.timezone.utc).strftime('%m%d-%H%M%S')
password=(pathlib.Path.home()/'lp-bench/pg_lp_bench.pass').read_text().strip()
env=os.environ.copy()
removed_names=sorted(k for k in env if k.startswith(('LP_','JSC_')))
for key in removed_names: env.pop(key)
(out/'environment-exclusions.json').write_text(json.dumps({'removed_names':removed_names,'explicit_JSC_usePollingTraps':'true','profiling_enabled':False},indent=2)+'\n')
env.update(FE_E2E_POSTGRES_MODE='native',FE_E2E_POSTGRES_PORT='5432',POSTGRES_USER='lp_bench',POSTGRES_PASSWORD=password,
 FE_E2E_RUN_SLUG=slug,E2E_TEMPLATE_DATABASE='fe_e2e_w2tpl_f9fde4aa_batch16fx_template',
 FE_E2E_REUSE_TEMPLATE_DATABASE='1',E2E_DATABASE_SNAPSHOT_RESTORE_WITH_DOCKER='0',E2E_ROUTING_ENGINE='lightpanda',
 E2E_ROUTING_REPORT=str(out/'lightpanda.json'),LIGHTPANDA_EXECUTABLE=str(binary),E2E_PLAYWRIGHT_WORKERS='1',LIGHTNINGPANDA_SHARED_ASSET_CACHE_MB='64')
env.update(JSC_usePollingTraps='true',E2E_TIMINGS='1',E2E_TIMING_DIR=str(out/'setup-timings'))
env['FE_E2E_REQUIRE_PREPARED_RUNTIME']='1'
env['PATH']='/opt/homebrew/bin:/opt/homebrew/opt/postgresql@17/bin:'+env['PATH']
pg_env=env.copy();pg_env['PGPASSWORD']=password
query="SELECT datname,pg_get_userbyid(datdba),datistemplate,datallowconn FROM pg_database WHERE pg_get_userbyid(datdba)='lp_bench' ORDER BY datname"
pg=['psql','-h','127.0.0.1','-p','5432','-U','lp_bench','-d','postgres','-X','-A','-t','-c',query]
def snapshot(name):
 p=subprocess.run(pg,env=pg_env,capture_output=True,text=True,check=True);(out/name).write_text(p.stdout)
def scrub(text):
 text=text.replace(password,'<redacted>') if password else text
 text=re.sub(r'eyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+','<redacted-jwt>',text)
 text=re.sub(r'(postgres(?:ql)?://[^:\s]+:)[^@\s]+@',r'\1<redacted>@',text)
 return text
snapshot('db-pre.txt')
meta={'comparison':proof,
 'tos_commit':'f9fde4aa0dce7d1d7a1ffebffe3306399973cd80','run_slug':slug,'template_database':env['E2E_TEMPLATE_DATABASE'],
 'binary':str(binary),'worktree':str(wt),'auth':'backend test-token -> localStorage; no UI login',
 'version_gate':'direct scripts/run-fe-e2e.mjs with playwright.routed.config.ts; same documented bypass as upstream baseline',
 'measurement':'frozen observer-free ABBA BAAB eight-process24-navigation matrix; setup excluded; original assertions/auth, shared64/private64, explicit JIT/polling; not fullsuite or independent24samples','lightningpanda_shared_asset_cache_mb':'64','expected_selected_cases':1,'specs':['e2e/configuration/profile-settings.e2e.ts']}
(out/'metadata.json').write_text(json.dumps(meta,indent=2)+'\n')
cmd=['perl','-e','alarm 1800; exec @ARGV','/usr/bin/time','-l','bun','--no-orphans','scripts/run-fe-e2e.mjs','run',
 '--config','/private/tmp/lp-final-readiness-93b1b3a/playwright.config.ts','/private/tmp/lp-final-readiness-93b1b3a/profile-nav.e2e.ts']
(out/'command.json').write_text(json.dumps(cmd,indent=2)+'\n')
started=time.monotonic()
with (out/'coverage.log').open('w') as log:
 p=subprocess.Popen(cmd,cwd=wt,env=env,stdout=subprocess.PIPE,stderr=subprocess.STDOUT,text=True)
 for line in p.stdout: log.write(scrub(line));log.flush()
 code=p.wait();log.write(f'\nexit={code}\n')
meta.update(exit_code=code,harness_wall_seconds=time.monotonic()-started)
(out/'metadata.json').write_text(json.dumps(meta,indent=2)+'\n')
snapshot('db-post.txt')
report=out/'lightpanda.json'
if report.exists():report.write_text(scrub(report.read_text()))
print(json.dumps({'exit_code':code,'run_slug':slug,'artifacts':str(out)}))
sys.exit(0)
