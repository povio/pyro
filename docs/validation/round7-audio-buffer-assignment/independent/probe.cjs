module.exports = async function probe(implementation) {
  const api = implementation ? (0, eval)(implementation)({activated:()=>true,timer:(delay, fn)=>setTimeout(fn,delay),clearTimer:clearTimeout}) : globalThis;
  const results = {};
  for (const [label, constructor, actions] of [
    ['empty',false,'start'], ['first',false,'a start'], ['nulls',false,'clear clear b start'],
    ['same',false,'a a start'], ['different',false,'a b start'], ['cleared',false,'a clear b start'],
    ['clearAfterStart',false,'a start clear'], ['rejectAfterStart',false,'a start b'],
    ['clearRejectAfterStart',false,'a start clear b'], ['lateFirst',false,'start a'],
    ['option',true,'b start'], ['optionClear',true,'clear b start'], ['nullOption',null,'b start'],
    ['clearDuringRender',false,'a start suspend clear b resume']]) {
    const ctx = new api.OfflineAudioContext(1,384,48000);
    const buffers = {};
    for(const [name, sign] of [['a',1],['b',-1]]) {
      const buffer = buffers[name] = ctx.createBuffer(1,384,48000);
      const data = buffer.getChannelData(0);
      for(let frame=0;frame<data.length;frame++) data[frame]=sign*((frame%7)+1)/8;
    }
    const node = constructor===false ? ctx.createBufferSource() : new api.AudioBufferSourceNode(ctx,{buffer:constructor?buffers.a:null});
    node.connect(ctx.destination);
    const getter = ()=> node.buffer===null?'null':node.buffer===buffers.a?'a':node.buffer===buffers.b?'b':'other';
    const history=[['initial',null,getter()]];let rendering;
    for(const op of actions.split(' ')) {
      let error=null;
      try {
        if(op==='start') node.start();
        else if(op==='suspend') {const pause=ctx.suspend(128/48000);rendering=ctx.startRendering();await pause;}
        else if(op==='resume') await ctx.resume();
        else node.buffer=buffers[op]??null;
      } catch(exception){error=[exception.name,exception.code];}
      history.push([op,error,getter()]);
    }
    const output=await (rendering??ctx.startRendering());
    results[label]={history,pcm:Array.from(output.getChannelData(0))};
  }
  return results;
};
