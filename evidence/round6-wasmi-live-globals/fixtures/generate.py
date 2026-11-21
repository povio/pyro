"""Small original, valid application-shaped numeric modules. These contain no linear memory."""
from pathlib import Path
import json
out=Path(__file__).resolve().parent
def u(n):
 b=[]
 while n>=128:b.append((n&127)|128);n>>=7
 return bytes(b+[n])
def vec(xs):return u(len(xs))+b''.join(xs)
def text(s):b=s.encode();return u(len(b))+b
def sec(n,data):return bytes([n])+u(len(data))+data
def fn(args,res):return b'\x60'+vec([bytes([x]) for x in args])+vec([bytes([x]) for x in res])
def exp(n,k,i):return text(n)+bytes([k])+u(i)
def bod(x):b=bytes([0,*x,11]);return u(len(b))+b
def imp(n,k,ds):return text('h')+text(n)+bytes([k,*ds])
def mod(types, imports,funcs,globals,exports,codes,start=None):
 sections=[sec(1,vec(types))]
 if imports:sections.append(sec(2,vec(imports)))
 if funcs:sections.append(sec(3,vec([u(x)for x in funcs])))
 if globals:sections.append(sec(6,vec([bytes(x)for x in globals])))
 if exports:sections.append(sec(7,vec(exports)))
 if start is not None:sections.append(sec(8,u(start)))
 if codes:sections.append(sec(10,vec([bod(x) for x in codes])))
 return bytes([0,97,115,109,1,0,0,0])+b''.join(sections)
types=[fn([], [127]),fn([127],[127]),fn([],[])]
inc=[35,0,65,1,106,36,0,35,0]
modules={
 'alias':mod(types,[imp('hook',0,[1]),imp('g',3,[127,1])],[0,0],[],[exp('inc',0,1),exp('notify',0,2),exp('g',3,0),exp('alias',3,0)],[inc,[35,0,16,0]]),
 'defined':mod(types,[],[0],[[127,1,65,0,11]],[exp('inc',0,0),exp('g',3,0),exp('alias',3,0)],[inc]),
 'start':mod(types,[imp('hook',0,[1]),imp('g',3,[127,1])],[2],[],[exp('g',3,0)],[[35,0,16,0,26]],1),
 'wide':mod(types,[imp('g',3,[126,1])],[2],[],[exp('bump',0,0),exp('g',3,0)],[[35,0,66,1,124,36,0]]),
 'primitive':mod(types,[imp('g',3,[127,0])],[],[],[exp('g',3,0),exp('alias',3,0)],[]),
 'bridge':mod(types,[imp('hook',0,[1])],[1],[],[exp('call',0,1)],[[32,0,16,0]]),
}
for n,b in modules.items():(out/f'{n}.wasm').write_bytes(b)
(out/'modules.js').write_text('const bytes=Object.fromEntries(Object.entries('+json.dumps({n:list(b)for n,b in modules.items()})+').map(([k,v])=>[k,Uint8Array.from(v)]));\n')
# Live JS gain parameter -> Wasm driver -> independent existing audio FIR kernel.
(out/'gain-driver.wasm').write_bytes(mod(types,[imp('kernel',0,[1]),imp('g',3,[127,1])],[0],[],[exp('run',0,1)],[[35,0,16,0]]))
# Host-only Table API assessment; this module is not run in shipped JSC.
(out/'table.wasm').write_bytes(mod(types,[imp('t',1,[112,1,1,4])],[1],[],[exp('t',1,0),exp('run',0,0)],[[32,0,65,0,17,1,0]]))
