const fs=require('fs'),path=require('path'),net=require('net'),cp=require('child_process'),os=require('os');
const cache=path.resolve(__dirname,"..","..",".."),pw=require(path.join(cache,'round6-reference-158/census/runner/node_modules/playwright-core'));
(async()=>{const [label,bin]=process.argv.slice(2),s=net.createServer();await new Promise(r=>s.listen(0,'127.0.0.1',r));const port=s.address().port;await new Promise(r=>s.close(r));
const proc=cp.spawn(bin,['serve','--port',String(port),'--host','127.0.0.1','--log-level','error'],{stdio:'ignore',env:{...process.env,WPT_CHROME_STATE_DIR:fs.mkdtempSync(path.join(os.tmpdir(),'qa-enter-'))}});let b;
try{const endpoint='http://127.0.0.1:'+port;let version;for(let i=0;i<100;i++){try{version=await(await fetch(endpoint+'/json/version')).json();break}catch{}await new Promise(r=>setTimeout(r,100))}
b=await pw.chromium.connectOverCDP(endpoint);const p=await b.newPage(),rows=[];
for(const [fixture,contents] of Object.entries({same:'<p><b>one</b><i>two</i></p>',siblings:'<p>one</p><p>two</p>',mixed:'<h1>one</h1><h2>two</h2>',quote:'<blockquote><p>one</p><p>two</p></blockquote>',inline:'<b>one</b><i>two</i>',list:'<dl><dt>one</dt><dd>two</dd></dl>',cell:'<table><tr><td>one</td><td>two</td></tr></table>',empty:'<p></p><p>two</p>',address:'<address>one<br>two</address>',island:'<p>one</p><div contenteditable=false>two</div>'})){
 await p.setContent('<section><div id=h contenteditable>'+contents+'</div></section>');
 for(const action of ['first','last','forward','backward','host-start','host-mid','host-end','host-contents','first-node']){
 rows.push(await p.evaluate(({fixture,action})=>{h.focus();const w=document.createTreeWalker(h,NodeFilter.SHOW_TEXT);let text=[],n;while(n=w.nextNode())text.push(n);const first=text[0],last=text.at(-1),s=getSelection();switch(action){case 'first':s.collapse(first,1);break;case 'last':s.collapse(last,1);break;case 'forward':s.setBaseAndExtent(first,1,last,last.length);break;case 'backward':s.setBaseAndExtent(last,last.length,first,1);break;case 'host-start':s.collapse(h,0);break;case 'host-mid':s.collapse(h,1);break;case 'host-end':s.collapse(h,h.childNodes.length);break;case 'host-contents':s.selectAllChildren(h);break;case 'first-node':s.setBaseAndExtent(h,0,h,1);break}
 const value=document.queryCommandValue('formatBlock');return {fixture,action,value,enabled:document.queryCommandEnabled('formatBlock'),text:s.toString(),html:h.innerHTML,undo:document.queryCommandEnabled('undo')};},{fixture,action}));
 }}
fs.writeFileSync(path.join(__dirname,label+'.json'),JSON.stringify({version,rows},null,2));
}finally{if(b)await b.close();proc.kill()}})().catch(e=>{console.error(e);process.exitCode=1});
