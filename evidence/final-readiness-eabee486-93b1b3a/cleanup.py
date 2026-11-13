import json,os,pathlib,subprocess
out=pathlib.Path('/private/tmp/pyro-final-readiness-93b1b3a')
meta=json.loads((out/'metadata.json').read_text());prefix=meta['template_database']
pre={line.split('|')[0] for line in (out/'db-pre.txt').read_text().splitlines()}
rows=[line.split('|') for line in (out/'db-post.txt').read_text().splitlines()]
env=os.environ.copy();env['PGPASSWORD']=(pathlib.Path.home()/'pyro-bench/pg_lp_bench.pass').read_text().strip()
base=['/opt/homebrew/bin/psql','-h','127.0.0.1','-p','5432','-U','pyro_bench','-d','postgres','-X','-A','-t','-v','ON_ERROR_STOP=1','-c']
with (out/'db-cleanup.txt').open('w') as log:
 TEMPLATE='fe_e2e_w2tpl_f9fde4aa_batch16fx_template'  # this fixture's retained W2 template (registered in the ledger by run1.sh)
 LEDGER={t['name'] for t in json.loads(pathlib.Path('/private/tmp/pyro-w2cov/templates-ledger.json').read_text())['templates']} | {TEMPLATE}
 for name,owner,istemplate,allowconn in rows:
  if name in LEDGER or name in pre: continue
  assert owner=='pyro_bench' and name.startswith(prefix+'_bun_e2e_'),name
  quoted='"'+name.replace('"','""')+'"'
  check="SELECT pg_get_userbyid(datdba) FROM pg_database WHERE datname='"+name.replace("'","''")+"'"
  actual=subprocess.check_output(base+[check],env=env,text=True).strip();assert actual=='pyro_bench'
  for sql in ['ALTER DATABASE '+quoted+' IS_TEMPLATE false','DROP DATABASE '+quoted]:
   p=subprocess.run(base+[sql],env=env,capture_output=True,text=True,check=True);log.write(p.stdout)
 query="SELECT datname,pg_get_userbyid(datdba),datistemplate,datallowconn FROM pg_database WHERE pg_get_userbyid(datdba)='pyro_bench' ORDER BY datname"
 (out/'db-post-cleanup.txt').write_text(subprocess.check_output(base+[query],env=env,text=True))
left=[l.split('|')[0] for l in (out/'db-post-cleanup.txt').read_text().splitlines()]
owners={l.split('|')[0]:l.split('|')[1] for l in (out/'db-post-cleanup.txt').read_text().splitlines()}
kept=sorted(n for n in left if n in LEDGER and owners[n]=='pyro_bench')
unknown=sorted(n for n in left if n not in kept)
assert not unknown,('unknown or unverified DBs left (not deleted):',unknown)
print('run DBs left: 0 | retained ledger templates:',kept)
