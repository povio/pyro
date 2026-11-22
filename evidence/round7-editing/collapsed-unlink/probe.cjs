const fs=require('fs'),path=require('path'),net=require('net'),cp=require('child_process'),os=require('os');
const cache=path.join(os.homedir(),'Library/Caches/lightningpanda'),pw=require(path.join(cache,'round6-reference-158/census/runner/node_modules/playwright-core'));
(async()=>{const [label,bin]=process.argv.slice(2),s=net.createServer();await new Promise(r=>s.listen(0,'127.0.0.1',r));const port=s.address().port;await new Promise(r=>s.close(r));
const proc=cp.spawn(bin,['serve','--port',String(port),'--host','127.0.0.1','--log-level','error'],{stdio:'ignore',env:{...process.env,WPT_CHROME_STATE_DIR:fs.mkdtempSync(path.join(os.tmpdir(),'qa-enter-'))}});let b;
try{const endpoint='http://127.0.0.1:'+port;let version;for(let i=0;i<100;i++){try{version=await(await fetch(endpoint+'/json/version')).json();break}catch{}await new Promise(r=>setTimeout(r,100))}
b=await pw.chromium.connectOverCDP(endpoint);const rows=[];
for(const shape of ['name','href','styled','nested','plain','locked','plaintext'])for(const placement of ['start','middle','end','outside','full','reverse','element'])for(const previous of [false,true]){
 const p=await b.newPage();const tag=shape==='plain'?'span':'a',attr=shape==='name'?'name="mark"':'href="https://example.test/"';
 await p.setContent('<!doctype html><div id=h contenteditable="'+(shape==='plaintext'?'plaintext-only':'true')+'">L<'+tag+' '+attr+(shape==='styled'?' style="font-weight:bold;color:red"':'')+(shape==='locked'?' contenteditable=false':'')+'>'+(shape==='nested'?'<i>abcdef</i>':'abcdef')+'</'+tag+'>R</div>');
 rows.push(await p.evaluate(({shape,placement,previous})=>{const h=document.querySelector('#h'),orig=h.firstElementChild,t=orig.firstElementChild?.firstChild||orig.firstChild,s=getSelection(),events=[];h.focus();if(previous){s.collapse(h.lastChild,1);document.execCommand('insertText',false,'X')}
 const n=placement==='outside'?h:placement==='element'?orig:t,lo=placement==='middle'?3:placement==='outside'?1:placement==='end'?6:0,hi=placement==='full'||placement==='reverse'?6:lo;
 s.setBaseAndExtent(n,placement==='reverse'?hi:lo,n,placement==='reverse'?lo:hi);for(const type of ['beforeinput','input'])h.addEventListener(type,e=>events.push([e.type,e.inputType,e.data]));
 const path=n=>{const a=[];while(n&&n!==h){if(!n.parentNode)return null;a.unshift([...n.parentNode.childNodes].indexOf(n));n=n.parentNode}return n?a:null};
 const snap=()=>({html:h.innerHTML,anchor:[path(s.anchorNode),s.anchorOffset],focus:[path(s.focusNode),s.focusOffset],text:s.toString(),direction:s.direction,enabled:document.queryCommandEnabled('unlink'),eligible:document.queryCommandEnabled('undo'),connected:[orig.isConnected,t.isConnected],events:[...events]});
 const before=snap(),returned=document.execCommand('unlink'),after=snap(),typed=document.execCommand('insertText',false,'Q'),afterTyping=snap(),undo=document.execCommand('undo'),restored=snap();return {shape,placement,previous,before,returned,after,typed,afterTyping,undo,restored};
 },{shape,placement,previous}));await p.close();}
fs.writeFileSync(path.join(__dirname,'scripts-'+label+'.json'),JSON.stringify({version,rows},null,2));
}finally{if(b)await b.close();proc.kill()}})().catch(e=>{console.error(e);process.exitCode=1});
