"""Read each exact Git tree and repeat the repository's lexical TLS inventory comparison."""
from pathlib import Path
import collections,json,re,subprocess
w=Path(__file__).resolve().parent;repo=w/'source';root='crates/pyro-webapi/src'
def git(*args,input=None):
 return subprocess.check_output(['git','-C',str(repo),*args],input=input)
def inspect(ref):
 tree=git('ls-tree','-r','-z',ref,root).split(b'\0');sources={}
 for line in tree:
  if line:
   meta,path=line.split(b'\t',1)
   if path.endswith(b'.rs'):sources[path.decode()]=meta.split()[2]
 proc=subprocess.Popen(['git','-C',str(repo),'cat-file','--batch'],stdin=subprocess.PIPE,stdout=subprocess.PIPE)
 found=[];classified=None
 for path,blob in sources.items():
  proc.stdin.write(blob+b'\n');proc.stdin.flush();header=proc.stdout.readline().split();data=proc.stdout.read(int(header[2])).decode();proc.stdout.read(1)
  if path.endswith('/thread_state.rs'):
   classified=re.findall(r'"([\w:]+)"',data.split('pub const STATICS: &[&str] = &[',1)[1].split('];',1)[0])
  p=Path(path);module=p.parent.name if p.stem=='mod' else p.stem;depth=None
  for line in data.splitlines():
   code=line.lstrip()
   if depth is None:
    if not code.startswith('thread_local!'):continue
    depth=0
   for name in re.findall(r'static ([A-Z0-9_]+)\s*:',code):found.append(module+'::'+name)
   depth+=code.count('{')+code.count('(')-code.count('}')-code.count(')')
   if depth<=0 and ('}' in code or ')' in code):depth=None
 proc.stdin.close();assert proc.wait()==0 and classified is not None
 return {'source':git('rev-parse',ref).decode().strip(),'missingClassification':sorted((collections.Counter(found)-collections.Counter(classified)).elements()),'listedButAbsent':sorted((collections.Counter(classified)-collections.Counter(found)).elements()),'webLocksInventory':sorted(x for x in found if x.startswith('web_locks::')),'webLocksClassification':sorted(x for x in classified if x.startswith('web_locks::'))}
out=[inspect(x) for x in ['f29218a28','a51615099','d5edcc2cd']]
for x in out:assert x['missingClassification']==['association::OWNERS','custom::CONSTRUCTIONS','structured_clone::WIRE_CHUNKS'] and not x['listedButAbsent'] and x['webLocksInventory']==x['webLocksClassification']
(w/'tls-manifest-readback.json').write_text(json.dumps(out,indent=2)+'\n');print(out[0])
