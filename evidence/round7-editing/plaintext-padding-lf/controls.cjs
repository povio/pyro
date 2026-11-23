const fs=require('fs'),path=require('path'),net=require('net'),cp=require('child_process'),os=require('os');
const cache=path.resolve(__dirname,"..",".."),pw=require(path.join(cache,'round6-reference-158/census/runner/node_modules/playwright-core'));
(async()=>{const [label,bin]=process.argv.slice(2),s=net.createServer();await new Promise(r=>s.listen(0,'127.0.0.1',r));const port=s.address().port;await new Promise(r=>s.close(r));
const proc=cp.spawn(bin,['serve','--port',String(port),'--host','127.0.0.1','--log-level','error'],{stdio:'ignore',env:{...process.env,WPT_CHROME_STATE_DIR:fs.mkdtempSync(path.join(os.tmpdir(),'qa-enter-'))}});let b;
try{const endpoint='http://127.0.0.1:'+port;let version;for(let i=0;i<100;i++){try{version=await(await fetch(endpoint+'/json/version')).json();break}catch{}await new Promise(r=>setTimeout(r,100))}
b=await pw.chromium.connectOverCDP(endpoint);const p=await b.newPage(),rows=[];

for (const mode of ['plaintext-only', 'true']) {
 for (const whiteSpace of ['normal','pre-line','pre-wrap']) {
  for (const [html,offset] of [['<b>\n</b>',0],['<b>A\n</b>',1],['<b>Q\n\n</b>',2],['<b>Q\n\n</b>R',2],['Q<b>\n</b>',0],['<b>🦊\n\n</b>',3]]) {
   await p.setContent('<div id="h" contenteditable="'+mode+'" style="white-space:'+whiteSpace+'">'+html+'</div>');
   rows.push(await p.evaluate(({mode,whiteSpace,html,offset})=>{
    const h=document.querySelector('#h'), t=h.querySelector('b').firstChild, s=getSelection();
    h.focus(); s.collapse(t,offset);
    const ok=document.execCommand('insertText',false,'x'), after=h.innerHTML;
    const caret=[s.anchorNode?.textContent,s.anchorOffset], undo=document.execCommand('undo');
    return {mode,whiteSpace,html,offset,ok,after,caret,undo,restored:h.innerHTML};
   },{mode,whiteSpace,html,offset}));
  }
 }
}
fs.writeFileSync(path.join(__dirname,label+'.json'),JSON.stringify({version,rows}));}finally{if(b)await b.close();proc.kill()}})().catch(e=>{console.error(e);process.exitCode=1});
