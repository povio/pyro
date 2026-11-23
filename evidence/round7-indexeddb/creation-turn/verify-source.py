import sys,pathlib,subprocess,hashlib,mmap
arm,exe,d=sys.argv[1:];p=pathlib.Path(d)/'src'
rev={'parent':'c8e966870c343cd719de9be711e0497a8830de9c','candidate':'400b8aaa869e90593c7e8bc645ef03c6f11a169b'}
with open(exe,'rb') as f,mmap.mmap(f.fileno(),0,access=mmap.ACCESS_READ) as buf:
 for name,sha in rev.items():
  src=subprocess.check_output(['git','-C',str(p),'show',sha+':crates/pyro-webapi/src/idb.js']);offset=buf.find(src);print(arm,name,hashlib.sha256(src).hexdigest(),offset);assert (offset!=-1)==(arm==name)
