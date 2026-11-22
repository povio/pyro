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
   const target=document.getElementById('invokee'),button=document.getElementById('invoker');const out={parsed:window.parsedLookup,bare:typeof invokee,windowNamed:window.invokee===target,association:button.commandForElement===target};
   const ev=[];target.addEventListener('command',e=>ev.push([e.type,e.command,e.source===button,e.cancelable,e.bubbles]));button.click();out.custom=ev;
   const dialog=document.createElement('dialog');dialog.id='dialogtarget';document.body.append(dialog);button.commandForElement=dialog;button.command='show-modal';button.click();out.dialogOpen=dialog.open;button.command='close';button.click();out.dialogClosed=!dialog.open;
   const details=document.createElement('details');details.innerHTML='<summary>Info</summary>More';document.body.append(details);button.commandForElement=details;out.details={};for(const cmd of ['open','close','toggle']){button.command=cmd;button.click();out.details[cmd]=[button.command,details.open]}
   const pop=document.createElement('div');pop.popover='manual';document.body.append(pop);button.commandForElement=pop;button.command='show-popover';const stop=e=>e.preventDefault();pop.addEventListener('command',stop);button.click();out.canceled=!pop.matches(':popover-open');pop.removeEventListener('command',stop);button.click();out.shown=pop.matches(':popover-open');button.command='hide-popover';button.click();out.hidden=!pop.matches(':popover-open');
   const other=document.createElement('div');other.id='freshlyNamedButtonTarget';document.body.append(other);out.dynamic=[typeof freshlyNamedButtonTarget,window.freshlyNamedButtonTarget===other];other.remove();out.afterRemoval=typeof freshlyNamedButtonTarget;
   return out;
 });
 fs.writeFileSync(path.join(w,label+'-commands.json'),JSON.stringify(values,null,2));
 }finally{if(b)await b.close();if(proc)proc.kill();server.close()}})().catch(e=>{console.error(e);process.exitCode=1});
