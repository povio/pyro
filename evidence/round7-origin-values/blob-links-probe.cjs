const http=require('node:http'),fs=require('node:fs'),net=require('node:net'),path=require('node:path'),cp=require('node:child_process'),os=require('node:os');
const w=__dirname,ref=path.join(path.dirname(w),'round6-reference-158/census'),pw=require(path.join(ref,'runner/node_modules/playwright-core'));
const [label,explicit]=process.argv.slice(2);
const server=http.createServer((req,res)=>{res.setHeader('Content-Type','text/html');res.end('<!doctype html><body style="margin:0">');});
const listen=s=>new Promise(r=>s.listen(0,'127.0.0.1',()=>r(s.address().port)));
(async()=>{const port=await listen(server),sock=net.createServer(),cdp=await listen(sock);await new Promise(r=>sock.close(r));let proc,b;
 try{const bin=explicit||path.join(ref,'launch-158');
 const tmp=fs.mkdtempSync(path.join(os.tmpdir(),'r7-svg-fixture-'));
 proc=cp.spawn(bin,['serve','--host','127.0.0.1','--port',String(cdp),'--log-level','error','--load-resources','stylesheet','--load-resources','iframe'],{stdio:'ignore',env:{...process.env,WPT_CHROME_STATE_DIR:tmp,WPT_CHROME_REFERENCE_MODE:'wpt-stable'}});
 for(let i=0;i<120;i++){try{if((await fetch('http://127.0.0.1:'+cdp+'/json/version')).ok)break}catch{}await new Promise(r=>setTimeout(r,100))}
 b=await pw.chromium.connectOverCDP('http://127.0.0.1:'+cdp);const page=await b.newPage();await page.goto('http://127.0.0.1:'+port+'/page');
 const values=await page.evaluate(async()=>{
   const out={surface:typeof Origin};if(typeof Origin!=='function')return out;
   const ns='http://www.w3.org/2000/svg',xlink='http://www.w3.org/1999/xlink';
   const anchor=document.createElement('a'),svg=document.createElementNS(ns,'a');
   const base=document.createElement('base');base.href='https://one.example/';document.head.prepend(base);
   anchor.href='leaf';
   out.relative=Origin.from(anchor).isSameOrigin(Origin.from(base.href));
   base.href='https://two.example/';anchor.setAttribute('href','other');
   out.changed=Origin.from(anchor).isSameOrigin(Origin.from(base.href));
   anchor.removeAttribute('href');
   try{Origin.from(anchor);out.removed='accepted'}catch(e){out.removed=e.name}
   svg.setAttributeNS(xlink,'xlink:href','https://fallback.example/');
   svg.setAttribute('href','https://preferred.example/');
   out.svg=[Origin.from(svg).isSameOrigin(Origin.from('https://preferred.example/'))];
   svg.removeAttribute('href');out.svg.push(Origin.from(svg).isSameOrigin(Origin.from('https://fallback.example/')));
   let gets=0;const url=new URL('https://record.example/');
   Object.defineProperty(url,'href',{get(){++gets;return 'https://wrong.example/'}});
   out.urlRecord=[Origin.from(url).isSameOrigin(Origin.from('https://record.example/')),gets];
   const opaqueChild=()=>{
     const context=Origin.from(globalThis),value=URL.createObjectURL(new Blob(['document-local bytes'])),record=new URL(value);
     const retained=document.createElement('a'),firstReadAfterRevoke=document.createElement('a');
     retained.href=value;firstReadAfterRevoke.href=value;
     const match=x=>Origin.from(x).isSameOrigin(context);
     const before=[match(value),match(record),match(retained)];
     URL.revokeObjectURL(value);
     const after=[match(value),match(record),match(retained),match(firstReadAfterRevoke)];
     record.href=value;
     return {opaque:context.opaque,before,after,reparsed:match(record)};
   };
   const frame=document.createElement('iframe');frame.sandbox='allow-scripts';
   const result=new Promise(resolve=>{
     let timer;
     function receive(e){if(e.source===frame.contentWindow){clearTimeout(timer);removeEventListener('message',receive);resolve(e.data)}}
     addEventListener('message',receive);
     timer=setTimeout(()=>{removeEventListener('message',receive);resolve({timeout:true})},3000);
   });
   frame.srcdoc='<script>try{parent.postMessage(('+opaqueChild.toString()+')(),"*")}catch(e){parent.postMessage({error:e.name,message:e.message},"*")}<\/script>';
   document.body.append(frame);out.blob=await result;frame.remove();return out;
 });
 fs.writeFileSync(path.join(w,label+'-blob-links.json'),JSON.stringify(values,null,2));
 }finally{if(b)await b.close();if(proc)proc.kill();server.close()}})().catch(e=>{console.error(e);process.exitCode=1});
