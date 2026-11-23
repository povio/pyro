const fs=require('fs'),path=require('path'),net=require('net'),cp=require('child_process'),os=require('os');
const cache=path.join(os.homedir(),'Library/Caches/lightningpanda'),pw=require(path.join(cache,'round6-reference-158/census/runner/node_modules/playwright-core'));
(async()=>{const [label,bin]=process.argv.slice(2),s=net.createServer();await new Promise(r=>s.listen(0,'127.0.0.1',r));const port=s.address().port;await new Promise(r=>s.close(r));
const proc=cp.spawn(bin,['serve','--port',String(port),'--host','127.0.0.1','--log-level','error'],{stdio:'ignore',env:{...process.env,WPT_CHROME_STATE_DIR:fs.mkdtempSync(path.join(os.tmpdir(),'qa-enter-'))}});let b;
try{const endpoint='http://127.0.0.1:'+port;let version;for(let i=0;i<100;i++){try{version=await(await fetch(endpoint+'/json/version')).json();break}catch{}await new Promise(r=>setTimeout(r,100))}
b=await pw.chromium.connectOverCDP(endpoint);const p=await b.newPage(),rows=[];
for(const target of ['p','div'])for(const sep of ['p','div'])for(const reverse of [false,true])for(const shape of ['all','part','last','tbody','row','cell','host','collapsed','sibling','nested']){
 const table='<table><caption>cap</caption><thead><tr><th>first</th></tr></thead><tbody><tr><td>foo</td><td>bar</td></tr></tbody></table>';
 const content=shape==='nested'?'<div>'+table+'</div>':shape==='sibling'?'before'+table+'after':table;
 await p.setContent('<!doctype html><div id=h contenteditable>'+content+'</div>');
 rows.push(await p.evaluate(({target,sep,reverse,shape})=>{const h=document.querySelector('#h'),s=getSelection();h.focus();document.execCommand('defaultParagraphSeparator',false,sep);
 const tb=h.querySelector('table');const n=({tbody:h.querySelector('tbody'),row:h.querySelector('tbody tr'),cell:h.querySelector('td'),host:h})[shape]||tb;
 const start=n,end=n,lo=shape==='last'?2:0,hi=shape==='collapsed'?0:shape==='part'?2:n.childNodes.length;s.setBaseAndExtent(reverse?end:start,reverse?hi:lo,reverse?start:end,reverse?lo:hi);
 const point=n=>{const a=[];while(n&&n!==h){if(!n.parentNode)return null;a.unshift([...n.parentNode.childNodes].indexOf(n));n=n.parentNode}return n?a:null};
 const snap=()=>({html:h.innerHTML,anchor:[point(s.anchorNode),s.anchorOffset],focus:[point(s.focusNode),s.focusOffset],direction:s.direction,text:s.toString(),value:document.queryCommandValue('formatBlock')});const before=snap(),result=document.execCommand('formatBlock',false,'<'+target+'>'),after=snap(),undo=document.execCommand('undo'),restored=snap();return {target,sep,reverse,shape,before,result,after,undo,restored};
 },{target,sep,reverse,shape}));}
fs.writeFileSync(path.join(__dirname,'scripts-'+label+'.json'),JSON.stringify({version,rows},null,2));
}finally{if(b)await b.close();proc.kill()}})().catch(e=>{console.error(e);process.exitCode=1});
