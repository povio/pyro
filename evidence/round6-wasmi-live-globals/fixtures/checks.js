function contract(test,assert,eq) {
  test('Constructed native numeric Global brand, reads and writes',()=>{
    for(const [type,input,expected] of [['i32',4294967298,2],['i64',(1n<<65n)+3n,3n],['f32',1/3,Math.fround(1/3)],['f64',1/3,1/3]]){
      const g=new WebAssembly.Global({value:type,mutable:true},input);
      eq(g.value,expected);eq(g.valueOf(),expected);eq(Object.prototype.toString.call(g),'[object WebAssembly.Global]');
      g.value=type==='i64'?7n:7;eq(g.value,type==='i64'?7n:7);
    }
  });
  test('Re-exported mutable Global retains identity across instances and mutation is live',()=>{
    const m=new WebAssembly.Module(bytes.alias),g=new WebAssembly.Global({value:'i32',mutable:true},10);
    const a=new WebAssembly.Instance(m,{h:{hook:x=>x,g}}), b=new WebAssembly.Instance(m,{h:{hook:x=>x,g}});
    eq(a.exports.g,g);eq(a.exports.alias,b.exports.g);eq(a.exports.inc(),11);eq(g.value,11);
    g.value=90;eq(b.exports.inc(),91);eq(a.exports.g.value,91);
  });
  test('Defined exports preserve aliases and distinct instances have distinct native globals',()=>{
    const m=new WebAssembly.Module(bytes.defined),a=new WebAssembly.Instance(m),b=new WebAssembly.Instance(m);
    eq(a.exports.g,a.exports.alias,"two exports of one defined global");assert(a.exports.g!==b.exports.g);a.exports.g.value=5;eq(a.exports.inc(),6);eq(b.exports.g.value,0);
    const link=new WebAssembly.Instance(new WebAssembly.Module(bytes.alias),{h:{hook:x=>x,g:a.exports.g}});
    eq(link.exports.g,a.exports.g,"imported defined global re-export");eq(link.exports.inc(),7);eq(a.exports.g.value,7);
  });
  test('Synchronous JS callback may re-enter the same instance and read/write its Global',()=>{
    const g=new WebAssembly.Global({value:'i32',mutable:true},2);let i;
    i=new WebAssembly.Instance(new WebAssembly.Module(bytes.alias),{h:{g,hook(x){eq(x,2);eq(g.value,2);g.value=9;return i.exports.inc()}}});
    eq(i.exports.notify(),10);eq(g.value,10);
  });
  test('Nested callback can return through another store and back to the original',()=>{
    const g=new WebAssembly.Global({value:'i32',mutable:true},4);let a;
    const proxy=new WebAssembly.Instance(new WebAssembly.Module(bytes.bridge),{h:{hook(x){g.value+=x;return a.exports.inc()}}});
    a=new WebAssembly.Instance(new WebAssembly.Module(bytes.alias),{h:{g,hook(){return proxy.exports.call(3)}}});
    eq(a.exports.notify(),8);eq(g.value,8);
  });
  test('A start import may access an already-live Global and call an existing export',()=>{
    const g=new WebAssembly.Global({value:'i32',mutable:true},17);
    const live=new WebAssembly.Instance(new WebAssembly.Module(bytes.alias),{h:{hook:x=>x,g}});
    let count=0;
    const linked=new WebAssembly.Instance(new WebAssembly.Module(bytes.start),{h:{g,hook(x){eq(x,g.value);count++;return live.exports.inc()}}});
    eq(linked.exports.g,g);eq(count,1);eq(g.value,18);
  });
  test('An immutable primitive import is wrapped once and refuses writes',()=>{
    const i=new WebAssembly.Instance(new WebAssembly.Module(bytes.primitive),{h:{g:12}});
    eq(i.exports.g,i.exports.alias,"two re-exports of one primitive import");eq(i.exports.g.value,12);
    let e;try{i.exports.g.value=15}catch(x){e=x}assert(e instanceof TypeError);eq(i.exports.g.value,12);
  });
  test('Wrong global type or mutability rejects without changing the supplied Global',()=>{
    const m=new WebAssembly.Module(bytes.alias);
    for(const g of [new WebAssembly.Global({value:'f64',mutable:true},9),new WebAssembly.Global({value:'i32'},9),9]){
      let e;try{new WebAssembly.Instance(m,{h:{g,hook:x=>x}})}catch(x){e=x}
      assert(e instanceof WebAssembly.LinkError);if(typeof g==='object')eq(g.value,9);
    }
  });
  test('Nested host exception identity is retained and subsequent entries still work',()=>{
    const marker={},g=new WebAssembly.Global({value:'i32',mutable:true},8);let active=true;let a;
    a=new WebAssembly.Instance(new WebAssembly.Module(bytes.alias),{h:{g,hook(){if(active){active=false;return a.exports.notify()}throw marker}}});
    let e;try{a.exports.notify()}catch(x){e=x}eq(e,marker);eq(a.exports.inc(),9);
  });

  test('Shared mutable i64 global keeps 64-bit browser and Wasm updates live',()=>{
    const m=new WebAssembly.Module(bytes.wide),g=new WebAssembly.Global({value:'i64',mutable:true},1n<<61n);
    const a=new WebAssembly.Instance(m,{h:{g}}),b=new WebAssembly.Instance(m,{h:{g}});
    a.exports.bump();eq(g.value,(1n<<61n)+1n);g.value=(1n<<62n)+3n;b.exports.bump();
    eq(a.exports.g.value,(1n<<62n)+4n);
  });
  test('Duplicate wrappers observe the same Wasm storage independently of JS wrapper identity',()=>{
    const {exports:e}=new WebAssembly.Instance(new WebAssembly.Module(bytes.defined));
    e.g.value=21;eq(e.alias.value,21);eq(e.inc(),22);eq(e.g.value,22);
  });
}
function asyncContract(register,eq) {
  register('Asynchronous compilation and byte/module instantiation preserve the supplied Global',async()=>{
    const g=new WebAssembly.Global({value:'i32',mutable:true},2);
    const module=await WebAssembly.compile(bytes.alias);
    const a=await WebAssembly.instantiate(module,{h:{g,hook:x=>x}});
    const {instance:b}=await WebAssembly.instantiate(bytes.alias,{h:{g,hook:x=>x}});
    eq(a.exports.inc(),3);eq(b.exports.inc(),4);eq(g.value,4);
  });
}
