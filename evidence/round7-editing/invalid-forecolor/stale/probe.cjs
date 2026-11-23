const fs=require('fs'),path=require('path'),net=require('net'),cp=require('child_process'),os=require('os');
const cache=path.join(os.homedir(),'Library/Caches/lightningpanda'),pw=require(path.join(cache,'round6-reference-158/census/runner/node_modules/playwright-core'));
(async()=>{const [label,bin]=process.argv.slice(2),s=net.createServer();await new Promise(r=>s.listen(0,'127.0.0.1',r));const port=s.address().port;await new Promise(r=>s.close(r));
const proc=cp.spawn(bin,['serve','--port',String(port),'--host','127.0.0.1','--log-level','error'],{stdio:'ignore',env:{...process.env,WPT_CHROME_STATE_DIR:fs.mkdtempSync(path.join(os.tmpdir(),'qa-enter-'))}});let b;
try{const endpoint='http://127.0.0.1:'+port;let version;for(let i=0;i<100;i++){try{version=await(await fetch(endpoint+'/json/version')).json();break}catch{}await new Promise(r=>setTimeout(r,100))}
b=await pw.chromium.connectOverCDP(endpoint);const rows=[];
const values=['potato quiche'];
for(const css of [false,true])for(const shape of ['replace-same','replace-markup','remove-ranges','detached-range','valid-in-host','valid-other-host','blur'])for(const value of values){
 const p=await b.newPage();
 await p.setContent(shape==='control'?'<input id=h value=abcdef>':'<div id=h '+(shape==='locked'?'contenteditable=false':shape==='plain'?'contenteditable=plaintext-only':'contenteditable')+'>foobarbaz</div>');
 rows.push(await p.evaluate(({css,shape,value})=>{const h=document.querySelector('#h'),s=getSelection(),events=[],text=h.firstChild;h.focus();
 if(shape==='control')h.setSelectionRange(1,4);else if(shape==='reverse')s.setBaseAndExtent(text,6,text,3);else s.setBaseAndExtent(text,shape==='entire'?0:3,text,shape==='entire'?9:shape==='caret'?3:6);
 document.execCommand('styleWithCSS',false,css);if(shape==='pending')document.execCommand('foreColor',false,'blue');if(shape==='setup'){document.execCommand('insertText',false,'xyz');s.setBaseAndExtent(h.firstChild,0,h.firstChild,2);}
 for(const name of ['beforeinput','input'])h.addEventListener(name,e=>events.push([e.type,e.inputType,e.data]));
 const point=n=>{const path=[];while(n&&n!==h){if(!n.parentNode)return null;path.unshift([...n.parentNode.childNodes].indexOf(n));n=n.parentNode}return n?path:null};
 const snap=()=>({rangeCount:s.rangeCount,anchorName:s.anchorNode?.nodeName??null,inHost:h.contains(s.anchorNode),hostConnected:h.isConnected,html:shape==='control'?h.value:h.innerHTML,value:document.queryCommandValue('foreColor'),enabled:document.queryCommandEnabled('foreColor'),undoable:document.queryCommandEnabled('undo'),anchor:[point(s.anchorNode),s.anchorOffset],focus:[point(s.focusNode),s.focusOffset],direction:s.direction,events:[...events],children:[...h.childNodes].map(n=>[n.nodeName,n.textContent]),connected:text?.isConnected??null});
 const before=snap(),returned=document.execCommand('foreColor',false,value),after=snap();
 if(shape==='replace-same')h.innerHTML=h.innerHTML;
 if(shape==='replace-markup')h.innerHTML='<b>external</b>';
 if(['remove-ranges','detached-range','valid-in-host','valid-other-host','blur'].includes(shape)){
 h.innerHTML='<b>external</b>';
 if(shape==='remove-ranges')s.removeAllRanges();
 if(shape==='detached-range'){const detached=document.createTextNode('abc');s.setBaseAndExtent(detached,0,detached,1);}
 if(shape==='valid-in-host')s.collapse(h.firstChild.firstChild,2);
 if(shape==='valid-other-host'){const other=document.createElement('div');other.contentEditable=true;other.textContent='other';document.body.appendChild(other);other.focus();s.collapse(other.firstChild,2);}
 if(shape==='blur')h.blur();
 }
 if(shape==='new-sibling')h.appendChild(document.createTextNode('!'));
 if(shape==='move-selection')s.collapse(h.firstChild,1);
 const intervening=snap(),undo=document.execCommand('undo'),restored=snap(),redo=document.execCommand('redo'),redone=snap();return {css,shape,value,before,returned,after,intervening,undo,restored,redo,redone};
 },{css,shape,value}));await p.close();
}
fs.writeFileSync(path.join(__dirname,'scripts-'+label+'.json'),JSON.stringify({version,rows},null,2));
}finally{if(b)await b.close();proc.kill()}})().catch(e=>{console.error(e);process.exitCode=1});
