const fs=require('fs'),path=require('path'),net=require('net'),cp=require('child_process'),os=require('os');
const cache=path.join(os.homedir(),'Library/Caches/lightningpanda'),pw=require(path.join(cache,'round6-reference-158/census/runner/node_modules/playwright-core'));
(async()=>{const [label,bin]=process.argv.slice(2),s=net.createServer();await new Promise(r=>s.listen(0,'127.0.0.1',r));const port=s.address().port;await new Promise(r=>s.close(r));
const proc=cp.spawn(bin,['serve','--port',String(port),'--host','127.0.0.1','--log-level','error'],{stdio:'ignore',env:{...process.env,WPT_CHROME_STATE_DIR:fs.mkdtempSync(path.join(os.tmpdir(),'qa-enter-'))}});let b;
try{const endpoint='http://127.0.0.1:'+port;let version;for(let i=0;i<100;i++){try{version=await(await fetch(endpoint+'/json/version')).json();break}catch{}await new Promise(r=>setTimeout(r,100))}
b=await pw.chromium.connectOverCDP(endpoint);const rows=[];
const values=['currentColor','inherit','initial','reset','invalid-value'];
for(const css of [false,true])for(const shape of ['font','span','styled','mixed','spaced'])for(const value of values){
 const p=await b.newPage();
 await p.setContent(shape==='control'?'<input id=h value=abcdef>':'<div id=h '+(shape==='locked'?'contenteditable=false':shape==='plain'?'contenteditable=plaintext-only':'contenteditable')+'>'+({font:'<font color=red>foo</font>',span:'<span style="color:red">foo</span>',styled:'<span style="color:red;font-style:italic">foo</span>',mixed:'<font color=red>foo</font><span style="color:blue">bar</span>',spaced:'<span>foo</span> <span>bar</span>'})[shape]+'</div>');
 rows.push(await p.evaluate(({css,shape,value})=>{const h=document.querySelector('#h'),s=getSelection(),events=[],text=h.firstChild;h.focus();
 s.setBaseAndExtent(h.firstChild.firstChild,0,h.lastChild.firstChild,3);
 document.execCommand('styleWithCSS',false,css);if(shape==='pending')document.execCommand('foreColor',false,'blue');if(shape==='setup'){document.execCommand('insertText',false,'xyz');s.setBaseAndExtent(h.firstChild,0,h.firstChild,2);}
 for(const name of ['beforeinput','input'])h.addEventListener(name,e=>events.push([e.type,e.inputType,e.data]));
 const point=n=>{const path=[];while(n&&n!==h){if(!n.parentNode)return null;path.unshift([...n.parentNode.childNodes].indexOf(n));n=n.parentNode}return n?path:null};
 const snap=()=>({html:shape==='control'?h.value:h.innerHTML,value:document.queryCommandValue('foreColor'),enabled:document.queryCommandEnabled('foreColor'),undoable:document.queryCommandEnabled('undo'),anchor:[point(s.anchorNode),s.anchorOffset],focus:[point(s.focusNode),s.focusOffset],direction:s.direction,events:[...events],children:[...h.childNodes].map(n=>[n.nodeName,n.textContent]),connected:text?.isConnected??null});
 const before=snap(),returned=document.execCommand('foreColor',false,value),after=snap(),undo=document.execCommand('undo'),restored=snap(),redo=document.execCommand('redo'),redone=snap();return {css,shape,value,before,returned,after,undo,restored,redo,redone};
 },{css,shape,value}));await p.close();
}
fs.writeFileSync(path.join(__dirname,'scripts-'+label+'.json'),JSON.stringify({version,rows},null,2));
}finally{if(b)await b.close();proc.kill()}})().catch(e=>{console.error(e);process.exitCode=1});
