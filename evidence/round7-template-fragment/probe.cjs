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
   const out={};
   const markup='<div><?start name="p">old<?end> tail</div><template for=p><b>new</b></template>';
   for(const mode of ['range','setHTMLUnsafe','appendHTMLUnsafe','prependHTMLUnsafe','innerHTML','parse']){
     const host=document.createElement('div');document.body.appendChild(host);
     if(mode==='range') {
        const range=document.createRange();range.selectNodeContents(host);
        const f=range.createContextualFragment(markup);host.appendChild(f);
     }else if(mode==='parse'){host.appendChild(document.importNode(new DOMParser().parseFromString(markup,'text/html').body.firstChild,true));}
     else if(mode==='innerHTML'){host.innerHTML=markup}
     else {if(typeof host[mode]!=='function'){out[mode]='missing';continue}host[mode](markup)}
     out[mode]=host.innerHTML;
   }
   const host=document.createElement('section');host.innerHTML='<p><?marker name="same"></p>';document.body.appendChild(host);
   const range=document.createRange();range.selectNodeContents(host);
   const isolated=range.createContextualFragment('<template for=same>outside</template>');out.outer=[host.firstChild.textContent,isolated.firstChild?.nodeName];
   const xml=new DOMParser().parseFromString('<root xmlns="http://www.w3.org/1999/xhtml"/>','application/xml');const xr=xml.createRange();xr.selectNodeContents(xml.documentElement);
   const frag=xr.createContextualFragment('<p><?marker name="a"?></p><template for="a">xml</template>');out.xml=[frag.childNodes.length,frag.lastChild.nodeName];
   return out;
 });
 fs.writeFileSync(path.join(w,label+'-probe.json'),JSON.stringify(values,null,2));
 }finally{if(b)await b.close();if(proc)proc.kill();server.close()}})().catch(e=>{console.error(e);process.exitCode=1});
