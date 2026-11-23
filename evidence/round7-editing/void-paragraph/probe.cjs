const fs=require('fs'),path=require('path'),net=require('net'),cp=require('child_process'),os=require('os');
const cache=path.join(os.homedir(),'Library/Caches/lightningpanda'),pw=require(path.join(cache,'round6-reference-158/census/runner/node_modules/playwright-core'));
(async()=>{const [label,bin]=process.argv.slice(2),s=net.createServer();await new Promise(r=>s.listen(0,'127.0.0.1',r));const port=s.address().port;await new Promise(r=>s.close(r));
const proc=cp.spawn(bin,['serve','--port',String(port),'--host','127.0.0.1','--log-level','error'],{stdio:'ignore',env:{...process.env,WPT_CHROME_STATE_DIR:fs.mkdtempSync(path.join(os.tmpdir(),'qa-enter-'))}});let b;
try{const endpoint='http://127.0.0.1:'+port;let version;for(let i=0;i<100;i++){try{version=await(await fetch(endpoint+'/json/version')).json();break}catch{}await new Promise(r=>setTimeout(r,100))}
b=await pw.chromium.connectOverCDP(endpoint);const p=await b.newPage(),rows=[];
for(const tag of ['img','input','embed','hr','br','wbr'])for(const container of ['div','h1','li'])for(const pos of ['only','leading','trailing']){
 const block='<'+container+'>'+(pos==='trailing'?'abc':'')+'<'+tag+'>'+(pos==='leading'?'abc':'')+'</'+container+'>';
 await p.setContent('<div id=h contenteditable>'+(container==='li'?'<ol>'+block+'</ol>':block)+'</div>');
 rows.push(await p.evaluate(({tag,container,pos})=>{const h=document.querySelector('#h'),orig=h.querySelector(tag),s=getSelection();h.focus();s.collapse(orig,0);const point=n=>{let a=[];while(n&&n!==h){if(!n.parentNode)return null;a.unshift([...n.parentNode.childNodes].indexOf(n));n=n.parentNode}return a};const snap=()=>({html:h.innerHTML,anchor:[point(s.anchorNode),s.anchorOffset],focus:[point(s.focusNode),s.focusOffset],original:orig.isConnected,voidChildren:orig.childNodes.length});const before=snap(),ok=document.execCommand('insertParagraph'),after=snap(),undo=document.execCommand('undo');return {tag,container,pos,before,ok,after,undo,restored:snap()};},{tag,container,pos}));}
fs.writeFileSync(path.join(__dirname,'styles-'+label+'.json'),JSON.stringify({version,rows},null,2))
}finally{if(b)await b.close();proc.kill()}})().catch(e=>{console.error(e);process.exitCode=1});
