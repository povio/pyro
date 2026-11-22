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
   const el=(tag,parent=document.body)=>parent.appendChild(document.createElement(tag));
   const first=el('form'),second=el('form');second.id='second';
   const fieldset=el('fieldset',first),legend=el('legend',fieldset),object=el('object',first);
   const out={implicit:[legend.form===first,object.form===first]};
   fieldset.setAttribute('form','second');object.setAttribute('form','second');
   out.explicit=[legend.form===second,object.form===second];
   const wrapper=el('div',fieldset);wrapper.appendChild(legend);out.indirect=legend.form===null;
   fieldset.appendChild(legend);const host=el('div');host.id='host';
   const root=host.attachShadow({mode:'closed',referenceTarget:'f'});const shadowForm=el('form',root);shadowForm.id='f';
   fieldset.setAttribute('form','host');object.setAttribute('form','host');
   out.closed=[legend.form===host,object.form===host];
   root.referenceTarget='missing';out.missing=[legend.form===null,object.form===null];
   return out;
 });
 fs.writeFileSync(path.join(w,label+'-form-probe.json'),JSON.stringify(values,null,2));
 }finally{if(b)await b.close();if(proc)proc.kill();server.close()}})().catch(e=>{console.error(e);process.exitCode=1});
