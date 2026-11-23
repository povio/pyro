const fs=require('fs'),path=require('path'),net=require('net'),cp=require('child_process'),os=require('os');
const cache=path.resolve(__dirname,"..",".."),pw=require(path.join(cache,'round6-reference-158/census/runner/node_modules/playwright-core'));
(async()=>{const [label,bin]=process.argv.slice(2),s=net.createServer();await new Promise(r=>s.listen(0,'127.0.0.1',r));const port=s.address().port;await new Promise(r=>s.close(r));
const proc=cp.spawn(bin,['serve','--port',String(port),'--host','127.0.0.1','--log-level','error'],{stdio:'ignore',env:{...process.env,WPT_CHROME_STATE_DIR:fs.mkdtempSync(path.join(os.tmpdir(),'qa-enter-'))}});let b;
try{const endpoint='http://127.0.0.1:'+port;let version;for(let i=0;i<100;i++){try{version=await(await fetch(endpoint+'/json/version')).json();break}catch{}await new Promise(r=>setTimeout(r,100))}
b=await pw.chromium.connectOverCDP(endpoint);const p=await b.newPage(),rows=[];


for(const prior of ['pre-wrap','normal','pre-line']) for(const hostTag of ['span','div']) for(const action of ['split','enter']){
 await p.setContent('<'+hostTag+' id="h" contenteditable="plaintext-only" style="white-space:pre-wrap"></'+hostTag+'>');
 rows.push(await p.evaluate(({prior,hostTag,action})=>{
  const host=document.querySelector('#h'),s=getSelection();host.focus();
  if(action==='split'){
    host.innerHTML='<i style="white-space:'+prior+'">a\n</i><b>\n</b>';s.collapse(host.lastChild.firstChild,0);
  } else {
    host.textContent='a';s.collapse(host.firstChild,1);document.execCommand('insertLineBreak');
  }
  const before=host.innerHTML;document.execCommand('insertText',false,'x');
  const after=host.innerHTML, caret=[s.anchorNode?.textContent,s.anchorOffset];document.execCommand('undo');
  return {prior,hostTag,action,before,after,caret,restored:host.innerHTML};
 },{prior,hostTag,action}));
}
fs.writeFileSync(path.join(__dirname,label+'.json'),JSON.stringify({version,rows}));}finally{if(b)await b.close();proc.kill()}})().catch(e=>{console.error(e);process.exitCode=1});
