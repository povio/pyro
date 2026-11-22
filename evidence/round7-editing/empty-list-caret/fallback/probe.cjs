const fs=require('fs'),path=require('path'),net=require('net'),cp=require('child_process'),os=require('os');
const cache=path.join(os.homedir(),'Library/Caches/lightningpanda'),pw=require(path.join(cache,'round6-reference-158/census/runner/node_modules/playwright-core'));
(async()=>{const [label,bin]=process.argv.slice(2),s=net.createServer();await new Promise(r=>s.listen(0,'127.0.0.1',r));const port=s.address().port;await new Promise(r=>s.close(r));
const proc=cp.spawn(bin,['serve','--port',String(port),'--host','127.0.0.1','--log-level','error'],{stdio:'ignore',env:{...process.env,WPT_CHROME_STATE_DIR:fs.mkdtempSync(path.join(os.tmpdir(),'qa-enter-'))}});let b;
try{const endpoint='http://127.0.0.1:'+port;let version;for(let i=0;i<100;i++){try{version=await(await fetch(endpoint+'/json/version')).json();break}catch{}await new Promise(r=>setTimeout(r,100))}
b=await pw.chromium.connectOverCDP(endpoint);const p=await b.newPage(),rows=[];
for(const tag of ['ul','ol'])for(const invalid of [false,true])for(const offset of [0,1]){
 await p.setContent('<div contenteditable id=h></div>');rows.push(await p.evaluate(({tag,invalid,offset})=>{const h=document.querySelector('#h'),outer=h.appendChild(document.createElement('p')),container=invalid?outer.appendChild(document.createElement('p')):outer,list=container.appendChild(document.createElement(tag)),li=list.appendChild(document.createElement('li'));li.appendChild(document.createElement('br'));h.focus();const s=getSelection();s.collapse(li,offset);
 const snap=()=>({html:h.innerHTML,connected:!!s.anchorNode?.isConnected,inside:h.contains(s.anchorNode),container:s.anchorNode?.nodeName,offset:s.anchorOffset});
 const before=snap(),typed=document.execCommand('insertText',false,'x'),undo=document.execCommand('undo'),restored=snap(),redo=document.execCommand('redo'),redone=snap();return {tag,invalid,offset,before,typed,undo,restored,redo,redone};
 },{tag,invalid,offset}));}
fs.writeFileSync(path.join(__dirname,'scripts-'+label+'.json'),JSON.stringify({version,rows},null,2));
}finally{if(b)await b.close();proc.kill()}})().catch(e=>{console.error(e);process.exitCode=1});
