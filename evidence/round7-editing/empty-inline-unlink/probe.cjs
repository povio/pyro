const fs=require('fs'),path=require('path'),net=require('net'),cp=require('child_process'),os=require('os');
const cache=path.join(os.homedir(),'Library/Caches/lightningpanda'),pw=require(path.join(cache,'round6-reference-158/census/runner/node_modules/playwright-core'));
(async()=>{const [label,bin]=process.argv.slice(2),s=net.createServer();await new Promise(r=>s.listen(0,'127.0.0.1',r));const port=s.address().port;await new Promise(r=>s.close(r));
const proc=cp.spawn(bin,['serve','--port',String(port),'--host','127.0.0.1','--log-level','error'],{stdio:'ignore',env:{...process.env,WPT_CHROME_STATE_DIR:fs.mkdtempSync(path.join(os.tmpdir(),'qa-enter-'))}});let b;
try{const endpoint='http://127.0.0.1:'+port;let version;for(let i=0;i<100;i++){try{version=await(await fetch(endpoint+'/json/version')).json();break}catch{}await new Promise(r=>setTimeout(r,100))}
b=await pw.chromium.connectOverCDP(endpoint);const rows=[];
for(const shape of ['href','name','span','b-i','two','br','image','text','space','nbsp','hidden','inline-block','border','locked','block','placeholder','prefix','suffix'])for(const reverse of [false,true])for(const previous of [false,true]){
 const p=await b.newPage(),fragment=({href:'<a href="/x"></a>',name:'<a name=foo></a>',span:'<span></span>','b-i':'<b><i></i></b>',two:'<a></a><i></i>',br:'<br>',image:'<img width=8 height=8>',text:'<a href="/x">X</a>',space:'<span> </span>',nbsp:'<span>&nbsp;</span>',hidden:'<span hidden>X</span>','inline-block':'<span style="display:inline-block;width:12px"></span>',border:'<span style="border:2px solid"></span>',locked:'<i contenteditable=false></i>',block:'<div></div>',placeholder:'<span><br></span>',prefix:'<span></span>',suffix:'<span></span>'})[shape];
 await p.setContent('<div id=h contenteditable>'+(shape==='prefix'?'':'L')+fragment+(shape==='suffix'?'':'R')+'</div>');
 rows.push(await p.evaluate(({shape,reverse,previous})=>{const placement=reverse?'reverse':'forward',h=document.querySelector('#h'),orig=h.firstElementChild,t=orig.firstChild||orig,s=getSelection(),events=[];h.focus();if(previous){s.collapse(h,0);document.execCommand('insertText',false,'Q')}
 const i=[...h.childNodes].indexOf(orig),end=i+(shape==='two'?2:1);s.setBaseAndExtent(h,reverse?end:i,h,reverse?i:end);for(const type of ['beforeinput','input'])h.addEventListener(type,e=>events.push([e.type,e.inputType,e.data]));
 const path=n=>{const a=[];while(n&&n!==h){if(!n.parentNode)return null;a.unshift([...n.parentNode.childNodes].indexOf(n));n=n.parentNode}return n?a:null};
 const snap=()=>({html:h.innerHTML,anchor:[path(s.anchorNode),s.anchorOffset],focus:[path(s.focusNode),s.focusOffset],text:s.toString(),direction:s.direction,enabled:document.queryCommandEnabled('unlink'),eligible:document.queryCommandEnabled('undo'),connected:[orig.isConnected,t.isConnected],events:[...events]});
 const before=snap(),returned=document.execCommand('unlink'),after=snap(),undo=document.execCommand('undo'),restored=snap(),redo=document.execCommand('redo'),redone=snap();return {shape,placement,previous,before,returned,after,undo,restored,redo,redone};
 },{shape,reverse,previous}));await p.close();}
fs.writeFileSync(path.join(__dirname,'scripts-'+label+'.json'),JSON.stringify({version,rows},null,2));
}finally{if(b)await b.close();proc.kill()}})().catch(e=>{console.error(e);process.exitCode=1});
