((native) => {
  'use strict';
  const modules = new WeakMap(), instances = new WeakMap(), globals = native.globalSlots;
  class CompileError extends Error {}
  class LinkError extends Error {}
  class RuntimeError extends Error {}
  Object.defineProperty(CompileError.prototype, 'name', {value:'CompileError', configurable:true,writable:true});
  Object.defineProperty(LinkError.prototype, 'name', {value:'LinkError', configurable:true,writable:true});
  Object.defineProperty(RuntimeError.prototype, 'name', {value:'RuntimeError', configurable:true,writable:true});
  const errors = { CompileError, LinkError, RuntimeError };
  function unpack([kind, value]) {
    if (kind === 'ok') return value;
    throw new errors[kind](value);
  }
  function handle(m) {
    const value = modules.get(m);
    if (!value) throw new TypeError('Expected a WebAssembly.Module');
    return value;
  }
  class Module {
    constructor(bytes) { modules.set(this, unpack(native.compile(bytes))); }
    static imports(m) { return handle(m).imports.map(({module,name,kind})=>({module,name,kind})); }
    static exports(m) { return handle(m).exports.map(({name,kind})=>({name,kind})); }
    static customSections(m, name) {
      if (arguments.length < 2) throw new TypeError('A section name is required');
      return handle(m).custom(String(name));
    }
  }
  function globalData(value) {
    const h=globals.get(value);
    if (!h) throw new TypeError('Expected a WebAssembly.Global');
    return h;
  }
  function convertGlobal(type,value) {
    return type==='i64' ? BigInt.asIntN(64,value).toString() : value;
  }
  function readGlobal(value) {
    const h=globalData(value);
    return h.type==='i64' ? BigInt(h.get()) : h.get();
  }
  class Global {
    constructor(descriptor, value=undefined) {
      if (!descriptor || (typeof descriptor!=='object' && typeof descriptor!=='function')) throw new TypeError('Expected GlobalDescriptor');
      const mutable=!!descriptor.mutable, type=`${descriptor.value}`;
      if (!['i32','i64','f32','f64'].includes(type)) throw new TypeError('Unsupported Wasm global type');
      if (value===undefined) value=type==='i64'?0n:0;
      globals.set(this,native.newGlobal(type,mutable,convertGlobal(type,value)));
    }
    get value(){return readGlobal(this)}
    set value(value){
      const h=globalData(this);
      if (!h.mutable) throw new TypeError('Global is immutable');
      h.set(convertGlobal(h.type,value));
    }
    valueOf(){return readGlobal(this)}
  }
  function importObject(value) {
    if (value !== undefined && (value === null || (typeof value !== 'object' && typeof value !== 'function')))
      throw new TypeError('Expected an import object');
  }
  class Instance {
    constructor(m, imports = undefined) {
      const h = handle(m);
      importObject(imports);
      // This array privately maps each module global index to its JS wrapper.
      // Referencing the same import, or exporting an index twice, preserves identity.
      const globalObjects = [];
      const functions = h.imports.map(({module,name,kind,valueType,mutable})=>{
        const ns = imports?.[module];
        if (ns === null || (typeof ns !== 'object' && typeof ns !== 'function')) throw new TypeError('Missing import module '+module);
        const value = ns[name];
        if (kind==='global') {
          const known=globals.get(value);
          globalObjects.push(known ? value : undefined);
          if (known) return known;
          if (mutable || (valueType==='i64' ? typeof value!=='bigint' : typeof value!=='number')) throw new LinkError('Expected a matching global import');
          return convertGlobal(valueType,value);
        }
        if (kind !== 'function') throw new LinkError('Unsupported Wasm import kind');
        if (typeof value !== 'function') throw new LinkError('Expected a function import '+name);
        return value;
      });
      const calls = unpack(h.start(functions));
      const exports = Object.create(null);
      for (const desc of h.exports) {
        const {name,kind,globalIndex}=desc;
        let value;
        if (kind==='global'){
          value=globalObjects[globalIndex];
          if (!value){
            value=Object.create(Global.prototype);
            globals.set(value,calls[name]);
            globalObjects[globalIndex]=value;
          }
        }else{
          const invoke=calls[name];
          value=(...args)=>unpack(invoke(functions,args));
        }
        Object.defineProperty(exports,name,{value,enumerable:true});
      }
      instances.set(this,Object.freeze(exports));
    }
    get exports() {
      if (!instances.has(this)) throw new TypeError('Expected a WebAssembly.Instance');
      return instances.get(this);
    }
  }
  for (const [cls,name] of [[Module,'Module'],[Instance,'Instance'],[Global,'Global']])
    Object.defineProperty(cls.prototype,Symbol.toStringTag,{value:'WebAssembly.'+name,configurable:true});
  function compile(bytes) {
    return new Promise((resolve,reject)=>{
      native.background(bytes, result=>{
        try {
          const m=Object.create(Module.prototype); modules.set(m,unpack(result)); resolve(m);
        } catch(e) { reject(e); }
      });
    });
  }
  function later(f) {
    return new Promise((resolve,reject)=>native.task(()=>{try{resolve(f());}catch(e){reject(e);}}));
  }
  async function instantiate(value, imports=undefined) {
    importObject(imports);
    if (modules.has(value)) return later(()=>new Instance(value,imports));
    const module=await compile(value);
    return { module, instance: new Instance(module,imports) };
  }
  const api={};
  for (const [name,value] of Object.entries({Module,Instance,Global,CompileError,LinkError,RuntimeError,compile,instantiate,
      validate(bytes){return native.validate(bytes);}}))
    Object.defineProperty(api,name,{value,writable:true,configurable:true});
  Object.defineProperty(api,Symbol.toStringTag,{value:'WebAssembly',configurable:true});
  Object.defineProperty(globalThis,'WebAssembly',{value:api,configurable:true,writable:true});
})
