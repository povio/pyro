const fs=require('fs'),path=require('path'),net=require('net'),cp=require('child_process'),os=require('os');
const cache=path.join(os.homedir(),'Library/Caches/lightningpanda'),pw=require(path.join(cache,'round6-reference-158/census/runner/node_modules/playwright-core'));
(async()=>{const [label,bin]=process.argv.slice(2),s=net.createServer();await new Promise(r=>s.listen(0,'127.0.0.1',r));const port=s.address().port;await new Promise(r=>s.close(r));
const proc=cp.spawn(bin,['serve','--port',String(port),'--host','127.0.0.1','--log-level','error'],{stdio:'ignore',env:{...process.env,WPT_CHROME_STATE_DIR:fs.mkdtempSync(path.join(os.tmpdir(),'qa-enter-'))}});let b;
try{const endpoint='http://127.0.0.1:'+port;let version;for(let i=0;i<100;i++){try{version=await(await fetch(endpoint+'/json/version')).json();break}catch{}await new Promise(r=>setTimeout(r,100))}
b=await pw.chromium.connectOverCDP(endpoint);const rows=[];
for(const order of [['plain','previous-list'],['previous-list','plain']]){
 const p=await b.newPage(); let previous=null;
 const record={order,steps:[]};
 for(const [index,shape] of order.entries()){
  const beforeSetup=await p.evaluate(()=>({eligible:document.queryCommandEnabled('undo'),html:document.querySelector('#h')?.innerHTML??null}));
  await p.setContent('<div id=h contenteditable>'+(shape==='previous-list'?'<ol><li>L</li></ol>':'')+'<p><br></p></div>');
  const host=await p.evaluateHandle(()=>document.querySelector('#h'));
  const data=await p.evaluate(({previous,shape,index})=>{
   const host=document.querySelector('#h'),sel=getSelection(),events=[];
   host.addEventListener('input',e=>events.push(e.inputType));host.focus();document.execCommand('defaultParagraphSeparator',false,'p');sel.collapse(host.lastChild,0);
   const snap=()=>({eligible:document.queryCommandEnabled('undo'),redoEligible:document.queryCommandEnabled('redo'),html:host.innerHTML,previousHtml:previous?.innerHTML??null,previousConnected:previous?.isConnected??null,sameHost:previous===host,selectionInHost:host.contains(sel.anchorNode),events:[...events]});
   const initial=snap(),actions=[];
   for(const cmd of ['insertOrderedList','insertOrderedList','undo','undo',...(index===0?['redo']:['undo'])]){
    const returned=document.execCommand(cmd);actions.push({cmd,returned,snapshot:snap()});
   }
   return {initial,actions};
  },{previous,shape,index});
  record.steps.push({shape,beforeSetup,...data});previous=host;
 }
 rows.push(record);await p.close();
}
fs.writeFileSync(path.join(__dirname,'scripts-'+label+'.json'),JSON.stringify({version,rows},null,2));
}finally{if(b)await b.close();proc.kill()}})().catch(e=>{console.error(e);process.exitCode=1});
