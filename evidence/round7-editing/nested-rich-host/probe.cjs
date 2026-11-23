const fs=require('fs'),path=require('path'),net=require('net'),cp=require('child_process'),os=require('os');
const cache=path.join(os.homedir(),'Library/Caches/lightningpanda'),pw=require(path.join(cache,'round6-reference-158/census/runner/node_modules/playwright-core'));
(async()=>{const [label,bin]=process.argv.slice(2),s=net.createServer();await new Promise(r=>s.listen(0,'127.0.0.1',r));const port=s.address().port;await new Promise(r=>s.close(r));
const proc=cp.spawn(bin,['serve','--port',String(port),'--host','127.0.0.1','--log-level','error'],{stdio:'ignore',env:{...process.env,WPT_CHROME_STATE_DIR:fs.mkdtempSync(path.join(os.tmpdir(),'qa-enter-'))}});let b;
try{const endpoint='http://127.0.0.1:'+port;let version;for(let i=0;i<100;i++){try{version=await(await fetch(endpoint+'/json/version')).json();break}catch{}await new Promise(r=>setTimeout(r,100))}
b=await pw.chromium.connectOverCDP(endpoint);const p=await b.newPage(),rows=[];
for(const innerMode of ['true','','plaintext-only'])for(const shape of ['locked','inherited','locked-target','cross'])for(const focus of ['outer','inner','control'])for(const range of ['forward','reverse','children']){
 await p.setContent('<div id=h contenteditable="plaintext-only">AB<section id=barrier '+(shape==='inherited'?'':'contenteditable=false')+'><div id=inner contenteditable="'+(shape==='locked-target'?'false':innerMode)+'">abcd</div></section>EF</div><input id=control value="outside">');
 rows.push(await p.evaluate(({innerMode,shape,focus,range})=>{const h=document.querySelector('#h'),inner=document.querySelector('#inner'),control=document.querySelector('#control'),s=getSelection(),t=inner.firstChild;
 h.focus();s.collapse(h.firstChild,1);if(focus==='inner')inner.focus();
 if(shape==='cross')s.setBaseAndExtent(h.firstChild,1,t,3);else if(range==='children')s.selectAllChildren(inner);else if(range==='reverse')s.setBaseAndExtent(t,3,t,1);else s.setBaseAndExtent(t,1,t,3);
 if(focus==='control')control.focus();
 const events=[];h.addEventListener('input',e=>events.push([e.target.id,e.inputType]));const point=n=>{const a=[];while(n&&n!==h){if(!n.parentNode)return null;a.unshift([...n.parentNode.childNodes].indexOf(n));n=n.parentNode}return n?a:null};
 const snap=()=>({html:h.innerHTML,active:document.activeElement.id,enabled:document.queryCommandEnabled('bold'),bold:document.queryCommandState('bold'),anchor:[point(s.anchorNode),s.anchorOffset],focus:[point(s.focusNode),s.focusOffset],direction:s.direction,text:s.toString(),connected:t.isConnected,control:control.value,events:[...events]});
 const before=snap(),returned=document.execCommand('bold'),after=snap(),undo=document.execCommand('undo'),restored=snap();return {innerMode,shape,focus,range,before,returned,after,undo,restored};
 },{innerMode,shape,focus,range}));}
fs.writeFileSync(path.join(__dirname,'scripts-'+label+'.json'),JSON.stringify({version,rows},null,2));
}finally{if(b)await b.close();proc.kill()}})().catch(e=>{console.error(e);process.exitCode=1});
