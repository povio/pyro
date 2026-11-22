async function probe() {
  const rows = {};
  const make = async (label, options) => {
    try { const ctx = new AudioContext(options); rows[label] = ctx.renderQuantumSize ?? 'absent'; await ctx.close(); }
    catch (error) { rows[label] = error.name; }
  };
  for (const [label, value] of Object.entries({omitted:undefined, default:'default',hardware:'hardware',one:1,odd:129,frac:255.9,zero:0,neg:-1,nan:NaN,inf:Infinity,wrap:2**32+257,negWrap:257-2**32,str:'128',nil:null,bool:true,obj:{valueOf(){return 257}},boxed:new Number(257),boxedString:new String('default'),array:[128],big:128n,sym:Symbol(),max:48000*6,above:48000*6+1})) {
    await make(label,{sampleRate:48000,renderSizeHint:value});
  }
  const gets=[]; await make('access',new Proxy({latencyHint:'interactive',renderSizeHint:257,sampleRate:48000},{get(target,key){ gets.push(key); return target[key]; }}));rows.gets=gets.filter(k=>['latencyHint','renderSizeHint','sampleRate'].includes(k));
  await make('inherited',Object.create({renderSizeHint:257}));
  await make('throwGetter',{get renderSizeHint(){throw new URIError()}, get sampleRate(){throw new EvalError()}});
  const desc=Object.getOwnPropertyDescriptor(BaseAudioContext.prototype,'renderQuantumSize');
  rows.descriptor=desc ? [desc.enumerable,desc.configurable,typeof desc.set,desc.get.length,Object.hasOwn(AudioContext.prototype,'renderQuantumSize')] : null;
  try { desc?.get.call({}); rows.brand='no throw'; } catch(error){rows.brand=error.name;}
  for (const rate of [32000,44100,48000,96000]) for(const quantum of [129,512]) {const ctx=new AudioContext({sampleRate:rate,renderSizeHint:quantum});rows['latency'+rate+'/'+quantum]=ctx.baseLatency;await ctx.close();}
  return rows;
}
