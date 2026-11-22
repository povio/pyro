const fs=require('fs'),path=require('path'),net=require('net'),cp=require('child_process'),os=require('os');
const cache=path.join(os.homedir(),'Library/Caches/lightningpanda'),pw=require(path.join(cache,'round6-reference-158/census/runner/node_modules/playwright-core'));
(async()=>{const [label,bin]=process.argv.slice(2),s=net.createServer();await new Promise(r=>s.listen(0,'127.0.0.1',r));const port=s.address().port;await new Promise(r=>s.close(r));
const proc=cp.spawn(bin,['serve','--port',String(port),'--host','127.0.0.1','--log-level','error'],{stdio:'ignore',env:{...process.env,WPT_CHROME_STATE_DIR:fs.mkdtempSync(path.join(os.tmpdir(),'qa-enter-'))}});let b;
try{const endpoint='http://127.0.0.1:'+port;let version;for(let i=0;i<100;i++){try{version=await(await fetch(endpoint+'/json/version')).json();break}catch{}await new Promise(r=>setTimeout(r,100))}
b=await pw.chromium.connectOverCDP(endpoint);const p=await b.newPage(),rows=[];
for(const prefix of [' ', 'abc ', 'abc  ', 'abc\u00a0'])for(const ws of ['normal','pre-wrap'])for(const key of ['Backspace','Delete','Alt+Backspace','Alt+Delete']){
 await p.setContent('<div id=h contenteditable style="white-space:'+ws+'"><p><b>'+prefix+'d</b></p><p>guard</p></div>');
 await p.evaluate(({key,prefix})=>{const h=document.querySelector('#h');h.focus();const t=h.querySelector('b').firstChild;window.orig=t;window.events=[];h.addEventListener('beforeinput',e=>events.push([e.type,e.inputType]));h.addEventListener('input',e=>events.push([e.type,e.inputType]));getSelection().collapse(t,prefix.length+(key.endsWith('Backspace')?1:0))},{key,prefix});
 await p.keyboard.press(key);
 rows.push(await p.evaluate(({prefix,ws,key})=>{const h=document.querySelector('#h'),s=getSelection();const point=n=>{let a=[];while(n&&n!==h){if(!n.parentNode)return null;a.unshift([...n.parentNode.childNodes].indexOf(n));n=n.parentNode}return a};const snap=()=>({html:h.innerHTML,caret:[point(s.anchorNode),s.anchorOffset],original:orig.isConnected});const after=snap(),editEvents=events.slice(),undo=document.execCommand('undo');return {prefix,ws,key,after,editEvents,undo,restored:snap()};},{key,prefix,ws}));}
fs.writeFileSync(path.join(__dirname,'styles-'+label+'.json'),JSON.stringify({version,rows},null,2))
}finally{if(b)await b.close();proc.kill()}})().catch(e=>{console.error(e);process.exitCode=1});
