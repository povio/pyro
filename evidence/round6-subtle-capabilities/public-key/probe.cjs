const fs=require('node:fs'),path=require('node:path'),cp=require('node:child_process'),http=require('node:http'),net=require('node:net');
const pw=require(path.resolve(__dirname,'../../../round6-reference-158/census/runner/node_modules/playwright-core'));
const [label,bin]=process.argv.slice(2);
const listen=s=>new Promise(r=>s.listen(0,'127.0.0.1',()=>r(s.address().port)));
(async()=>{let browser,proc;const server=http.createServer((req,res)=>{res.setHeader('Content-Type','text/html');res.end('<!doctype html><title>Crypto API control</title>')});
 try{
 const port=await listen(server);
 if(bin==='chrome')browser=await pw.chromium.launch({executablePath:path.resolve(__dirname,'../../../round6-reference-158/Google Chrome.app/Contents/MacOS/Google Chrome'),headless:true});
 else{const sock=net.createServer(),bp=await listen(sock);await new Promise(r=>sock.close(r));proc=cp.spawn(bin,['serve','--host','127.0.0.1','--port',String(bp),'--log-level','error'],{stdio:'ignore'});const host='http://127.0.0.1:'+bp;for(let i=0;i<120;i++){try{if((await fetch(host+'/json/version')).ok)break}catch{}await new Promise(r=>setTimeout(r,100));}browser=await pw.chromium.connectOverCDP(host)}
 const page=await browser.newPage();await page.goto('http://127.0.0.1:'+port+'/');
 const results=await page.evaluate(async()=>{
  const subtle=crypto.subtle,results={};
  const bytes=b=>Array.from(new Uint8Array(b)).join(',');
  async function test(name,fn){try{results[name]=await fn()}catch(e){results[name]={error:e.name}}}
  const ec={name:'ECDSA',namedCurve:'P-256'};
  for(const specs of [[ec,['sign','verify'],['verify']],[{name:'ECDH',namedCurve:'P-384'},['deriveBits'],[]],['Ed25519',['sign','verify'],['verify']],['X25519',['deriveBits'],[]],[{name:'RSA-OAEP',hash:'SHA-256',publicExponent:new Uint8Array([1,0,1]),modulusLength:1024},['encrypt','decrypt'],['encrypt']]]) {
   const [alg,usages,pub]=specs,key=typeof alg==='string'?alg:alg.name;
   await test('nonextractable-'+key,async()=>{const pair=await subtle.generateKey(alg,false,usages);const derived=await subtle.getPublicKey(pair.privateKey,pub);return derived instanceof CryptoKey && derived.extractable && derived.type==='public' && derived.usages.join(',')===pub.join(',') && bytes(await subtle.exportKey('spki',derived))===bytes(await subtle.exportKey('spki',pair.publicKey));});
  }
  await test('concurrent-imported',async()=>{const pair=await subtle.generateKey(ec,true,['sign','verify']);
   const material=await subtle.exportKey('pkcs8',pair.privateKey),spki=bytes(await subtle.exportKey('spki',pair.publicKey));
   const hidden=await subtle.importKey('pkcs8',material,ec,false,['sign']);
   const keys=await Promise.all(Array.from({length:20},()=>subtle.getPublicKey(hidden,['verify'])));
   return new Set(keys).size===20 && (await Promise.all(keys.map(k=>subtle.exportKey('spki',k)))).every(x=>bytes(x)===spki)});
  await test('wrong-key-type-order',async()=>{const pair=await subtle.generateKey(ec,false,['sign','verify']),order=[];
   const p=subtle.getPublicKey(pair.publicKey,['verify']).catch(e=>order.push(e.name));queueMicrotask(()=>order.push('microtask'));
   await p;return order.join(',')});
  await test('bad-usage-order',async()=>{const pair=await subtle.generateKey(ec,false,['sign','verify']),order=[];
   const p=subtle.getPublicKey(pair.privateKey,['encrypt']).catch(e=>order.push(e.name));queueMicrotask(()=>order.push('microtask'));
   await p;return order.join(',')});
  await test('wrong-receiver',async()=>{let p;try {p=SubtleCrypto.prototype.getPublicKey.call({},{},[]);}catch(e){return 'threw '+e.name}
   try{await p;return false}catch(e){return e.name==='TypeError'}});
  await test('same-origin-frame',async()=>{const f=document.createElement('iframe');document.body.append(f);
   try{const api=f.contentWindow.crypto.subtle,pair=await api.generateKey(ec,false,['sign','verify']);
   const local=await subtle.getPublicKey(pair.privateKey,['verify']);
   return local instanceof CryptoKey && bytes(await subtle.exportKey('spki',local))===bytes(await api.exportKey('spki',pair.publicKey))}finally{f.remove()}});
  return results;
 });
 fs.writeFileSync(path.join(__dirname,label+'.json'),JSON.stringify(results,null,2));console.log(label,JSON.stringify(results));
 }finally{if(browser)await browser.close();if(proc)proc.kill();server.close()}
})().catch(e=>{console.error(e);process.exitCode=1});
