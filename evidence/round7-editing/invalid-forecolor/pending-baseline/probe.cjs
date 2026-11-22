const fs=require('fs'),path=require('path'),net=require('net'),cp=require('child_process'),os=require('os');
const cache=path.join(os.homedir(),'Library/Caches/lightningpanda'),pw=require(path.join(cache,'round6-reference-158/census/runner/node_modules/playwright-core'));
(async()=>{const [label,bin]=process.argv.slice(2),s=net.createServer();await new Promise(r=>s.listen(0,'127.0.0.1',r));const port=s.address().port;await new Promise(r=>s.close(r));
const proc=cp.spawn(bin,['serve','--port',String(port),'--host','127.0.0.1','--log-level','error'],{stdio:'ignore',env:{...process.env,WPT_CHROME_STATE_DIR:fs.mkdtempSync(path.join(os.tmpdir(),'qa-enter-'))}});let b;
try{const endpoint='http://127.0.0.1:'+port;let version;for(let i=0;i<100;i++){try{version=await(await fetch(endpoint+'/json/version')).json();break}catch{}await new Promise(r=>setTimeout(r,100))}
b=await pw.chromium.connectOverCDP(endpoint);const rows=[];
const values=['potato quiche','#f'];
for(const css of [false,true])for(const shape of ['bold','italic','fontSize','hiliteColor'])for(const value of values){
 const p=await b.newPage();
 await p.setContent(shape==='control'?'<input id=h value=abcdef>':'<div id=h '+(shape==='locked'?'contenteditable=false':shape==='plain'?'contenteditable=plaintext-only':'contenteditable')+'><span style="color:green">abcdef</span></div>');
 rows.push(await p.evaluate(({css,shape,value})=>{const h=document.querySelector('#h'),s=getSelection(),events=[],text=h.firstChild?.firstChild;h.focus();
 if(shape==='control')h.setSelectionRange(1,4);else if(shape==='reverse')s.setBaseAndExtent(text,4,text,1);else s.setBaseAndExtent(text,1,text,1);
 document.execCommand('styleWithCSS',false,css);document.execCommand(shape,false,shape==='fontSize'?'5':shape==='hiliteColor'?'yellow':'');document.execCommand('foreColor',false,'blue');if(shape==='history')document.execCommand('insertText',false,'X');
 for(const name of ['beforeinput','input'])h.addEventListener(name,e=>events.push([e.type,e.inputType,e.data]));
 const point=n=>{const path=[];while(n&&n!==h){if(!n.parentNode)return null;path.unshift([...n.parentNode.childNodes].indexOf(n));n=n.parentNode}return n?path:null};
 const snap=()=>({html:shape==='control'?h.value:h.innerHTML,value:document.queryCommandValue('foreColor'),enabled:document.queryCommandEnabled('foreColor'),undoable:document.queryCommandEnabled('undo'),anchor:[point(s.anchorNode),s.anchorOffset],focus:[point(s.focusNode),s.focusOffset],direction:s.direction,events:[...events],connected:text?.isConnected??null});
 const before=snap(),returned=true,after=snap(),typed=document.execCommand('insertText',false,'Q'),afterTyping=snap(),undo=document.execCommand('undo'),restored=snap();return {css,shape,value,before,returned,after,typed,afterTyping,undo,restored};
 },{css,shape,value}));await p.close();
}
fs.writeFileSync(path.join(__dirname,'scripts-'+label+'.json'),JSON.stringify({version,rows},null,2));
}finally{if(b)await b.close();proc.kill()}})().catch(e=>{console.error(e);process.exitCode=1});
