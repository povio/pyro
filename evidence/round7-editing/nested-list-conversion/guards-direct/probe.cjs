const fs=require('fs'),path=require('path'),net=require('net'),cp=require('child_process'),os=require('os');
const cache=path.join(os.homedir(),'Library/Caches/lightningpanda'),pw=require(path.join(cache,'round6-reference-158/census/runner/node_modules/playwright-core'));
(async()=>{const [label,bin]=process.argv.slice(2),s=net.createServer();await new Promise(r=>s.listen(0,'127.0.0.1',r));const port=s.address().port;await new Promise(r=>s.close(r));
const proc=cp.spawn(bin,['serve','--port',String(port),'--host','127.0.0.1','--log-level','error'],{stdio:'ignore',env:{...process.env,WPT_CHROME_STATE_DIR:fs.mkdtempSync(path.join(os.tmpdir(),'qa-enter-'))}});let b;
try{const endpoint='http://127.0.0.1:'+port;let version;for(let i=0;i<100;i++){try{version=await(await fetch(endpoint+'/json/version')).json();break}catch{}await new Promise(r=>setTimeout(r,100))}
b=await pw.chromium.connectOverCDP(endpoint);const p=await b.newPage(),rows=[];
for(const cmd of ['insertOrderedList','insertUnorderedList'])for(const reverse of [false,true])for(const shape of ['deep','partial','two-branches','locked-sibling','attr-item','opposite-inner','heading']){
 const src=cmd==='insertOrderedList'?'ul':'ol',dst=src==='ul'?'ol':'ul',inner=shape==='mixed'?dst:src;
 const part=(shape==='deep'?'<'+src+'><li>deep</li></'+src+'>':'');
 const fragment='<'+src+'><li'+(shape==='attr-item'?' id="outer" style="color:red"':'')+'>'+(shape==='heading'?'<b>foo</b>':'foo')+'<'+(shape==='opposite-inner'?dst:src)+'><li>bar'+part+'</li></'+(shape==='opposite-inner'?dst:src)+'></li><li'+(shape==='locked-sibling'?' contenteditable=false':'')+'>baz'+(shape==='two-branches'?'<'+src+'><li>tail</li></'+src+'>':'')+'</li></'+src+'>';
 await p.setContent('<div id=h contenteditable>'+fragment+'</div>');rows.push(await p.evaluate(({cmd,shape,reverse})=>{const h=document.querySelector('#h'),s=getSelection(),outer=h.firstChild,sub=outer.querySelector('ol,ul'),w=document.createTreeWalker(h,NodeFilter.SHOW_TEXT),nodes={};let t;while(t=w.nextNode())nodes[t.data]=t;
 const first=nodes.foo,last=nodes.deep||nodes.tail||nodes.bar,lo=shape==='partial'?1:0,hi=shape==='partial'?2:last.length;
 h.focus();s.setBaseAndExtent(reverse?last:first,reverse?hi:lo,reverse?first:last,reverse?lo:hi);
 const point=n=>{const p=[];while(n&&n!==h){if(!n.parentNode)return null;p.unshift([...n.parentNode.childNodes].indexOf(n));n=n.parentNode}return n?p:null};
 const snap=()=>({html:h.innerHTML,state:document.queryCommandState(cmd),value:document.queryCommandValue(cmd),ol:document.queryCommandState('insertOrderedList'),ul:document.queryCommandState('insertUnorderedList'),anchor:[point(s.anchorNode),s.anchorOffset],focus:[point(s.focusNode),s.focusOffset],direction:s.direction,text:s.toString(),connected:[outer,sub,first,last].map(n=>n.isConnected)});
 const before=snap(),ok=document.execCommand(cmd),after=snap(),again=document.execCommand(cmd),twice=snap(),undo=document.execCommand('undo'),onceUndo=snap(),undo2=document.execCommand('undo'),restored=snap();return {cmd,reverse,shape,before,ok,after,again,twice,undo,onceUndo,undo2,restored};
 },{cmd,shape,reverse}));}
fs.writeFileSync(path.join(__dirname,'scripts-'+label+'.json'),JSON.stringify({version,rows},null,2));
}finally{if(b)await b.close();proc.kill()}})().catch(e=>{console.error(e);process.exitCode=1});
