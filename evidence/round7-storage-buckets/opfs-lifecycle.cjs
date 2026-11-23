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
   const m=navigator.storageBuckets,out={surface:!!m};if(!m)return out;
   const err=async f=>{try{await f();return 'ok'}catch(e){return e.name}};
   const first=await m.open('files');const root=await first.getDirectory(),file=await root.getFileHandle('f',{create:true});const wr=await file.createWritable();await wr.write('uncommitted text');
   out.before=[(await file.getFile()).size,(await first.estimate()).usage>=0];
   out.deletion=await err(()=>m.delete('files'));const second=await m.open('files');
   out.commit=await err(()=>wr.close());out.old=[await err(()=>root.getFileHandle('f')),await err(()=>file.getFile()),await err(()=>first.getDirectory())];
   const fresh=await second.getDirectory();out.fresh=[await err(()=>fresh.getFileHandle('f')),await fresh.isSameEntry(root)];
   await m.delete('files');
   return out;
 });
 fs.writeFileSync(path.join(w,label+'-opfs-lifecycle.json'),JSON.stringify(values,null,2));
 }finally{if(b)await b.close();if(proc)proc.kill();server.close()}})().catch(e=>{console.error(e);process.exitCode=1});
