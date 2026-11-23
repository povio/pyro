const fs=require('fs'),path=require('path'),net=require('net'),cp=require('child_process'),os=require('os');
const cache=path.join(os.homedir(),'Library/Caches/lightningpanda'),pw=require(path.join(cache,'round6-reference-158/census/runner/node_modules/playwright-core'));
(async()=>{const [label,bin]=process.argv.slice(2),s=net.createServer();await new Promise(r=>s.listen(0,'127.0.0.1',r));const port=s.address().port;await new Promise(r=>s.close(r));
const proc=cp.spawn(bin,['serve','--port',String(port),'--host','127.0.0.1','--log-level','error'],{stdio:'ignore',env:{...process.env,WPT_CHROME_STATE_DIR:fs.mkdtempSync(path.join(os.tmpdir(),'qa-enter-'))}});let b;
try{const endpoint='http://127.0.0.1:'+port;let version;for(let i=0;i<100;i++){try{version=await(await fetch(endpoint+'/json/version')).json();break}catch{}await new Promise(r=>setTimeout(r,100))}
b=await pw.chromium.connectOverCDP(endpoint);const p=await b.newPage(),rows=[];
for(const source of ['<b>abc</b>','<b><i>abc</i></b>','<span style="color:red">abc</span>','<a href="#x"><b>abc</b></a>','<span id="carrier" class="red">abc</span>'])for(const markup of ['<u>X</u>','<b>X</b>','X'])for(const pos of [0,1,3]){
 await p.setContent('<style>.red{color:red}</style><div id=h contenteditable>'+source+'</div>');
 rows.push(await p.evaluate(({source,markup,pos})=>{const h=document.querySelector('#h');h.focus();let orig=h.firstChild,t=orig;while(t.firstChild)t=t.firstChild;const s=getSelection();s.collapse(t,pos);const snap=()=>({html:h.innerHTML,anchor:s.anchorNode?.nodeName,offset:s.anchorOffset,original:orig.isConnected,textConnected:t.isConnected,bold:document.queryCommandState('bold'),underline:document.queryCommandState('underline')});const before=snap(),ok=document.execCommand('insertHTML',false,markup),after=snap(),undo=document.execCommand('undo');return {source,markup,pos,before,ok,after,undo,restored:snap()};},{source,markup,pos}));}
fs.writeFileSync(path.join(__dirname,'styles-'+label+'.json'),JSON.stringify({version,rows},null,2))
}finally{if(b)await b.close();proc.kill()}})().catch(e=>{console.error(e);process.exitCode=1});
