(async () => {
  const portTimes=[],cloneTimes=[],bytes=new Uint8Array(8192).fill(37);
  for(let sample=0;sample<8;sample++){
    const p=new MessageChannel(),start=performance.now();
    await new Promise((ok,fail)=>{
      let count=0;
      p.port2.onmessage=e=>{
        if(e.data.i!==count || e.data.bytes[17]!==37) {fail(Error('message payload'));return}
        if(++count===128)ok();
      };
      for(let i=0;i<128;i++)p.port1.postMessage({i,bytes});
    });
    const end=performance.now();p.port1.close();p.port2.close();
    const before=performance.now();let sum=0;
    for(let i=0;i<512;i++)sum+=structuredClone({bytes,i:i+1}).bytes[17];
    if(sum!==512*37)throw Error('clone data');
    if(sample){portTimes.push(end-start);cloneTimes.push(performance.now()-before);}
  }
  return {portTimes,cloneTimes};
})()
