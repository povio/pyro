onmessage=async({data:{algorithm,key,ciphertext,shared}})=>{
  try{
    const recovered=new Uint8Array(await crypto.subtle.decapsulateBits(algorithm,key,ciphertext));
    postMessage({match:recovered.length===shared.length&&recovered.every((x,i)=>x===shared[i]),extractable:key.extractable,kind:key.type});
  }catch(e){postMessage({error:e.name});}
};
