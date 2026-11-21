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
  const subtle=crypto.subtle,name='ChaCha20-Poly1305',hex=s=>Uint8Array.from(s.match(/../g),x=>parseInt(x,16)),show=b=>Array.from(new Uint8Array(b),x=>x.toString(16).padStart(2,'0')).join('');
  const secret=hex('808182838485868788898a8b8c8d8e8f909192939495969798999a9b9c9d9e9f');
  const opts={name,iv:hex('070000004041424344454647'),additionalData:hex('50515253c0c1c2c3c4c5c6c7')};
  const message=new TextEncoder().encode("Ladies and Gentlemen of the class of '99: If I could offer you only one tip for the future, sunscreen would be it.");
  const expected='d31a8d34648e60db7b86afbc53ef7ec2a4aded51296e08fea9e2b5a736ee62d63dbea45e8ca9671282fafb69da92728b1a71de0a9e060b2905d6a5b67ecd3b3692ddbd7f2d778b8c9803aee328091b58fab324e4fad675945585808b4831d7bc3ff4def08e4b7a9de576d26586cec64b61161ae10b594f09e26a7e902ecbd0600691';
  const results={};
  async function test(k,f){try{results[k]=await f()}catch(e){results[k]={exception:e.name}}}
  await test('rfc8439',async()=>{const k=await subtle.importKey('raw-secret',secret,name,true,['encrypt','decrypt']);const c=await subtle.encrypt(opts,k,message);return show(c)===expected && show(await subtle.decrypt(opts,k,c))===show(message)});
  await test('authenticated-data',async()=>{const k=await subtle.importKey('raw-secret',secret,name,false,['decrypt']);try{await subtle.decrypt({...opts,additionalData:new Uint8Array([1])},k,hex(expected));return false}catch(e){return e.name==='OperationError'}});
  await test('jwk-roundtrip',async()=>{const original=await subtle.generateKey(name,true,['encrypt','decrypt']);const jwk=await subtle.exportKey('jwk',original);const k=await subtle.importKey('jwk',jwk,name,true,['decrypt']);return jwk.alg==='C20P' && original.algorithm.length===undefined && show(await subtle.exportKey('raw-secret',original))===show(await subtle.exportKey('raw-secret',k))});
  await test('key-wrap',async()=>{const wrap=await subtle.importKey('raw-secret',secret,name,false,['wrapKey','unwrapKey']);const k=await subtle.generateKey({name:'AES-GCM',length:128},true,['encrypt']);const envelope=await subtle.wrapKey('raw-secret',k,wrap,opts);const loaded=await subtle.unwrapKey('raw-secret',envelope,wrap,opts,'AES-GCM',true,['encrypt']);return show(await subtle.exportKey('raw',loaded))===show(await subtle.exportKey('raw',k))});
  await test('derived-key',async()=>{const k=await subtle.importKey('raw',secret,'HKDF',false,['deriveKey','deriveBits']);const kd={name:'HKDF',hash:'SHA-256',salt:new Uint8Array([1,2]),info:new Uint8Array([3,4])};const derived=await subtle.deriveKey(kd,k,name,true,['encrypt']);return show(await subtle.deriveBits(kd,k,256))===show(await subtle.exportKey('raw-secret',derived))});
  await test('classic-public-format',async()=>{const pair=await subtle.generateKey({name:'ECDSA',namedCurve:'P-256'},true,['sign','verify']);return show(await subtle.exportKey('raw-public',pair.publicKey))===show(await subtle.exportKey('raw',pair.publicKey))});
  await test('usage-before-format',async()=>{try {await subtle.importKey('raw-private',secret,name,false,['sign']);return false;}catch(e){return e.name==='SyntaxError'}});
  return results;
 });
 fs.writeFileSync(path.join(__dirname,label+'.json'),JSON.stringify(results,null,2));console.log(label,JSON.stringify(results));
 }finally{if(browser)await browser.close();if(proc)proc.kill();server.close()}
})().catch(e=>{console.error(e);process.exitCode=1});
