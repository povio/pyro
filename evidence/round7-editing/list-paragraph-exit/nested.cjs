const fs=require('fs'),path=require('path'),net=require('net'),cp=require('child_process'),os=require('os');
const cache=path.join(os.homedir(),'Library/Caches/lightningpanda'),pw=require(path.join(cache,'round6-reference-158/census/runner/node_modules/playwright-core'));
(async()=>{const [label,bin]=process.argv.slice(2),s=net.createServer();await new Promise(r=>s.listen(0,'127.0.0.1',r));const port=s.address().port;await new Promise(r=>s.close(r));
const proc=cp.spawn(bin,['serve','--port',String(port),'--host','127.0.0.1','--log-level','error'],{stdio:'ignore',env:{...process.env,WPT_CHROME_STATE_DIR:fs.mkdtempSync(path.join(os.tmpdir(),'qa-enter-'))}});let b;
try{const endpoint='http://127.0.0.1:'+port;let version;for(let i=0;i<100;i++){try{version=await(await fetch(endpoint+'/json/version')).json();break}catch{}await new Promise(r=>setTimeout(r,100))}
b=await pw.chromium.connectOverCDP(endpoint);const p=await b.newPage(),rows=[];
for(const parent of ['outerItem','outerList','div','hostList'])for(const pos of ['only','middle'])for(const kind of ['br','innerBR','text','locked']){
 const li='<li class="target">'+(kind==='text'?'abc':kind==='locked'?'<span contenteditable=false>lock</span>':'<br>')+'</li>',list='<ul class="inner">'+(pos==='middle'?'<li>prev</li>':'')+li+(pos==='middle'?'<li>next</li>':'')+'</ul>';
 const fragment=parent==='outerItem'?'<ol><li>outer'+list+'</li></ol>':parent==='outerList'?'<ol><li>outer</li>'+list+'</ol>':parent==='div'?'<div>'+list+'</div>':list; await p.setContent(parent==='hostList'?fragment.replace('<ul','<ul id=h contenteditable'): '<div id=h contenteditable>'+fragment+'</div>');
 rows.push(await p.evaluate(({parent,pos,kind})=>{const h=document.querySelector('#h'),li=h.querySelector('.target'),s=getSelection();h.focus();s.collapse(kind==='innerBR'?li.firstChild:li,0);const snap=()=>({html:h.innerHTML,node:s.anchorNode?.nodeName,off:s.anchorOffset,connected:li.isConnected});const before=snap();document.execCommand('insertParagraph');const after=snap(),undo=document.execCommand('undo');return {parent,pos,kind,before,after,undo,restored:snap()};},{parent,pos,kind}));}
fs.writeFileSync(path.join(__dirname,'nested-'+label+'.json'),JSON.stringify({version,rows},null,2))
}finally{if(b)await b.close();proc.kill()}})().catch(e=>{console.error(e);process.exitCode=1});
