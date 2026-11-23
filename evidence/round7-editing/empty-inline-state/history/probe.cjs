const fs=require('fs'),path=require('path'),net=require('net'),cp=require('child_process'),os=require('os');
const cache=path.join(os.homedir(),'Library/Caches/lightningpanda'),pw=require(path.join(cache,'round6-reference-158/census/runner/node_modules/playwright-core'));
(async()=>{const [label,bin]=process.argv.slice(2),s=net.createServer();await new Promise(r=>s.listen(0,'127.0.0.1',r));const port=s.address().port;await new Promise(r=>s.close(r));
const proc=cp.spawn(bin,['serve','--port',String(port),'--host','127.0.0.1','--log-level','error'],{stdio:'ignore',env:{...process.env,WPT_CHROME_STATE_DIR:fs.mkdtempSync(path.join(os.tmpdir(),'qa-enter-'))}});let b;
try{const endpoint='http://127.0.0.1:'+port;let version;for(let i=0;i<100;i++){try{version=await(await fetch(endpoint+'/json/version')).json();break}catch{}await new Promise(r=>setTimeout(r,100))}
b=await pw.chromium.connectOverCDP(endpoint);const rows=[];
for(const css of [false,true])for(const reverse of [false,true])for(const repeat of [false,true])for(const shape of ['b','i','bi','ib','bold-left']){
 const p=await b.newPage();const frag=({b:'<b></b>',i:'<i></i>',bi:'<b><i></i></b>',ib:'<i><b></b></i>',span:'<span></span>',br:'<br>',locked:'<b contenteditable=false></b>','bold-left':'<i></i>','bold-right':'<b></b>',edges:'<span></span>'})[shape];
 await p.setContent('<div id=h contenteditable>'+(shape==='bold-left'?'<b>foo</b>':shape==='edges'?'':'foo')+frag+(shape==='bold-right'?'<b>baz</b>':'baz')+'</div>');
 rows.push(await p.evaluate(({css,reverse,repeat,shape})=>{const h=document.querySelector('#h'),s=getSelection(),offset=shape==='edges'?0:1,el=h.childNodes[offset],events=[];h.focus();document.execCommand('styleWithCSS',false,css);
 s.setBaseAndExtent(h,reverse?offset+1:offset,h,reverse?offset:offset+1);h.oninput=e=>events.push(e.inputType);
 const point=n=>{const p=[];while(n&&n!==h){if(!n.parentNode)return null;p.unshift([...n.parentNode.childNodes].indexOf(n));n=n.parentNode}return n?p:null;};
 const snap=()=>{const w=document.createTreeWalker(h,NodeFilter.SHOW_TEXT),glyphs=[];let t;while(t=w.nextNode())for(const ch of t.data)glyphs.push([ch,getComputedStyle(t.parentElement).fontWeight,getComputedStyle(t.parentElement).fontStyle]);return {html:h.innerHTML,bold:document.queryCommandState('bold'),italic:document.queryCommandState('italic'),anchor:[point(s.anchorNode),s.anchorOffset],focus:[point(s.focusNode),s.focusOffset],direction:s.direction,text:s.toString(),connected:el.isConnected,eligible:document.queryCommandEnabled('undo'),events:[...events],glyphs}};
 const before=snap(),ok=document.execCommand('bold'),after=snap(),again=repeat?document.execCommand('bold'):null,pretype=snap(),insert=document.execCommand('insertText',false,'Q'),typed=snap(),undo=document.execCommand('undo'),restored=snap(),redo=document.execCommand('redo'),redone=snap();return {redo,redone,css,reverse,repeat,shape,before,ok,after,again,pretype,insert,typed,undo,restored}
 },{css,reverse,repeat,shape}));await p.close();}
fs.writeFileSync(path.join(__dirname,'scripts-'+label+'.json'),JSON.stringify({version,rows},null,2));
}finally{if(b)await b.close();proc.kill()}})().catch(e=>{console.error(e);process.exitCode=1});
