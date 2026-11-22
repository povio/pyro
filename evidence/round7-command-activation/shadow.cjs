const http=require('node:http'),fs=require('node:fs'),net=require('node:net'),path=require('node:path'),cp=require('node:child_process'),os=require('node:os');
const w=__dirname,ref=path.join(path.dirname(w),'round6-reference-158/census'),pw=require(path.join(ref,'runner/node_modules/playwright-core'));
const [label,explicit]=process.argv.slice(2);
const markup='<!doctype html><body><button type="button" commandfor="invokee" command="--preview" id="invoker">go</button><div id="invokee"></div><script>window.parsedLookup=[typeof invokee,window.invokee===document.getElementById("invokee")];</script>';
const server=http.createServer((req,res)=>{res.setHeader('Content-Type','text/html');res.end(markup)});
const listen=s=>new Promise(r=>s.listen(0,'127.0.0.1',()=>r(s.address().port)));
(async()=>{const port=await listen(server),sock=net.createServer(),cdp=await listen(sock);await new Promise(r=>sock.close(r));let proc,b;
 try{const bin=explicit||path.join(ref,'launch-158');
 const tmp=fs.mkdtempSync(path.join(os.tmpdir(),'r7-audio-'));
 proc=cp.spawn(bin,['serve','--host','127.0.0.1','--port',String(cdp),'--log-level','error','--load-resources','stylesheet','--load-resources','iframe'],{stdio:'ignore',env:{...process.env,WPT_CHROME_STATE_DIR:tmp,WPT_CHROME_REFERENCE_MODE:'wpt-stable'}});
 for(let i=0;i<120;i++){try{if((await fetch('http://127.0.0.1:'+cdp+'/json/version')).ok)break}catch{}await new Promise(r=>setTimeout(r,100))}
 b=await pw.chromium.connectOverCDP('http://127.0.0.1:'+cdp);const page=await b.newPage();await page.goto('http://127.0.0.1:'+port+'/page');
 const values=await page.evaluate(async()=>{
   const out={};const host=document.createElement('div');document.body.append(host);const root=host.attachShadow({mode:'open'});
   const b=document.createElement('button'),t=document.createElement('div');root.append(b,t);b.commandForElement=t;b.command='--demo';
   const seen=[];let retained;const snap=(key,e)=>{seen.push([key,e.composed,e.cancelable,e.bubbles,e.source===b?'button':e.source===host?'host':'other']);retained=e};
   host.addEventListener('command',e=>snap('host',e),true);root.addEventListener('command',e=>snap('root',e),true);t.addEventListener('command',e=>snap('target',e));
   b.click();out.activation=seen.slice();out.after=retained?.source===host?'host':retained?.source===b?'button':'other';
   const synth=new CommandEvent('command',{source:b});out.synthetic=[synth.composed,synth.source===host?'host':synth.source===b?'button':'other'];
   seen.length=0;const composed=new CommandEvent('command',{source:b,composed:true});t.dispatchEvent(composed);out.explicit=seen.slice();
   const plain=document.createElement('div'),button=document.createElement('button');document.body.append(button,plain);button.commandForElement=plain;
   let before=0;plain.addEventListener('command',e=>before++);button.command='show-popover';button.click();out.inapplicablePopover=[before,plain.matches(':popover-open')];
   const dialog=document.createElement('dialog');document.body.append(dialog);button.commandForElement=dialog;button.command='show-modal';button.remove();button.click();out.detachedAction=dialog.open;if(dialog.open)dialog.close();
   button.commandForElement=t;out.inaccessibleRef=button.commandForElement===null;
   return out;
 });
 fs.writeFileSync(path.join(w,label+'-shadow.json'),JSON.stringify(values,null,2));
 }finally{if(b)await b.close();if(proc)proc.kill();server.close()}})().catch(e=>{console.error(e);process.exitCode=1});
