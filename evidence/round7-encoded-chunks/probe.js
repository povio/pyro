(async()=>{
 const rows={};
 function trial(name,body){try{rows[name]=body()}catch(e){rows[name]='throws '+e.name}}
 for(const C of [globalThis.EncodedAudioChunk,globalThis.EncodedVideoChunk]){
  if(!C)continue;
  const make=o=>new C({type:'key',timestamp:-10,data:new Uint8Array([1,2,3]),...o});
  const use=f=>{const b=new Uint8Array(f.byteLength);f.copyTo(b);return Array.from(b).join(',')};
  trial(C.name+' data snapshots',()=>{const a=new Uint8Array([9,1,2,8]);let v=make({data:new DataView(a.buffer,1,2)});a[1]=0;let dest=new Uint8Array(4).fill(5);v.copyTo(new DataView(dest.buffer,1,2));return Array.from(dest).join(',')});
  trial(C.name+' transfer two',()=>{let a=new Uint8Array([4,5]),b=new ArrayBuffer(5);let v=make({data:a,transfer:[a.buffer,b]});return [a.buffer.byteLength,b.byteLength,use(v)]});
  trial(C.name+' duplicate',()=>{let a=new ArrayBuffer(4);try{make({transfer:[a,a]})}catch(e){return [e.name,a.byteLength]}});
  trial(C.name+' invalid transfer element',()=>make({transfer:[{}]}).byteLength);
  trial(C.name+' empty string transfer',()=>make({transfer:''}).byteLength);
  trial(C.name+' getter detaches data',()=>{const a=new Uint8Array([1,2]);return new C({data:a,get duration(){structuredClone(a.buffer,{transfer:[a.buffer]});return 1},timestamp:0,type:'key'}).byteLength});
  trial(C.name+' memo',()=>{let a=make({});Object.defineProperties(a,{type:{get(){throw Error()}},copyTo:{get(){throw Error()}}});let b=structuredClone([a,a]);return [b[0]===b[1],b[0] instanceof C,use(b[0])]});
  trial(C.name+' invalid transfer of chunk',()=>{let a=make({});try{structuredClone(a,{transfer:[a]})}catch(e){return e.name}});
  trial(C.name+' order',()=>{let log=[];new C(new Proxy({type:'key',timestamp:7,data:new Uint8Array(1)},{get:(o,k)=>(log.push(String(k)),o[k])}));return log});
  trial(C.name+' negative duration',()=>make({duration:-1}).byteLength);
  trial(C.name+' missing type',()=>make({type:undefined}).byteLength);
 }
 console.log(JSON.stringify(rows));
})()
