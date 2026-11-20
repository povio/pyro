import json,gzip,pathlib,re,hashlib,sys
r=pathlib.Path(sys.argv[1]) if len(sys.argv)>1 else pathlib.Path.home()/'Library/Caches/lightningpanda/round6-noncss-measure'; o=r/'merged-aa0268/batch/readback/work'; p=r/'platform-batch';g=r/'css-reconciliation-572c';inv=r.parent/'round6-inventory'
load=lambda f:json.loads(f.read_bytes())
urls=set(load(r/'merged-aa0268/batch/extra.json')+load(p/'selection.json')+load(p/'extra.json')+load(p/'loss-urls.json')+load(p/'a414-css-selection.json')+[t['url'] for t in load(g/'rebase-a414-extra.json')['results']])
def read_records(filename):
 dec=json.JSONDecoder()
 with gzip.open(filename,'rt') as f:
  buf=f.read(1<<17);start=re.match(r'\s*\{\s*"results"\s*:\s*\[',buf);assert start;buf=buf[start.end():]
  while True:
   buf=buf.lstrip(' \r\n\t,')
   if buf.startswith(']'):return
   try:v,end=dec.raw_decode(buf)
   except ValueError:
    extra=f.read(1<<17)
    if not extra:raise
    buf+=extra;continue
   yield v;buf=buf[end:]
for label in ('stable','canary'):
 out={}
 for z in read_records(inv/f'report-{label}.download'):
  u=z['test'].lstrip('/')
  if u not in urls:continue
  assert u not in out and not z.get('subsuite');out[u]={'status':z['status'],'subtests':[[t['name'],t['status']] for t in z.get('subtests',[])]}
 with gzip.open(o/f'{label}.json.gz','wt') as f:json.dump(out,f,separators=(',',':'))
 print(label,len(out),'of',len(urls))
def walk(t,p=''):
 for k,v in t.items():
  path=p+k
  if isinstance(v,dict):yield from walk(v,path+'/')
  else:
   for url,_ in v[1:]:
    u=(url or path).lstrip('/')
    if u in urls:yield u,[path,v[0]]
pin=dict(walk(load(inv/'MANIFEST-pinned.json')['items']['testharness']))
stable=dict(walk(load(inv/'MANIFEST-stable.json')['items']['testharness']))
with gzip.open(o/'sources.json.gz','wt') as f:json.dump({u:[*pin.get(u,[None,None]),u in pin and stable.get(u)==pin[u]] for u in sorted(urls)},f)
