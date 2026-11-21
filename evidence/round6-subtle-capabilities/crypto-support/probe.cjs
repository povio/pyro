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
  const results={};
  if(typeof SubtleCrypto.supports!=='function') return {available:false};
  const supports=(...args)=>SubtleCrypto.supports(...args);
  const observe=f=>{try{return f()}catch(e){return {exception:e.name}}};
  results.available=true;results.arity=SubtleCrypto.supports.length;
  results.invalidIv=supports('encrypt',{name:'AES-CBC',iv:new Uint8Array(8)});
  results.bigIntDictionary=observe(()=>supports('encrypt',{name:'AES-CTR',counter:new Uint8Array(16),length:64n}));
  results.coercedBigInt=observe(()=>supports('encrypt',{name:'AES-CTR',counter:new Uint8Array(16),length:{valueOf(){return 64n}}}));
  results.badRsa=supports('generateKey',{name:'RSA-PSS',modulusLength:2048,publicExponent:new Uint8Array([2]),hash:'SHA-256'});
  results.kdfWrap=supports('wrapKey','AES-KW','HKDF');
  const trace=[];
  const other={get name(){trace.push('name');return 'HMAC'},get hash(){trace.push('hash');return 'SHA-256'}};
  results.canDerive=supports('deriveKey',{name:'HKDF',hash:'SHA-256',salt:new Uint8Array([3]),info:new Uint8Array()},other);
  results.additionalReads=trace;
  const cipher={name:'ChaCha20-Poly1305',iv:new Uint8Array(12)};
  results.cipher=supports('encrypt',cipher);
  if(results.cipher){
    const k=await crypto.subtle.generateKey(cipher.name,false,['encrypt','decrypt']);
    const msg=new Uint8Array([12,34,56]),ct=await crypto.subtle.encrypt(cipher,k,msg);
    results.actualRoundTrip=Array.from(new Uint8Array(await crypto.subtle.decrypt(cipher,k,ct)));
  }
  return results;
 });
 fs.writeFileSync(path.join(__dirname,label+'.json'),JSON.stringify(results,null,2));console.log(label,JSON.stringify(results));
 }finally{if(browser)await browser.close();if(proc)proc.kill();server.close()}
})().catch(e=>{console.error(e);process.exitCode=1});
