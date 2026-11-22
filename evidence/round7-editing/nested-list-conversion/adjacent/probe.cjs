const fs=require('fs'),path=require('path'),net=require('net'),cp=require('child_process'),os=require('os');
const cache=path.join(os.homedir(),'Library/Caches/lightningpanda'),pw=require(path.join(cache,'round6-reference-158/census/runner/node_modules/playwright-core'));
(async()=>{const [label,bin]=process.argv.slice(2),s=net.createServer();await new Promise(r=>s.listen(0,'127.0.0.1',r));const port=s.address().port;await new Promise(r=>s.close(r));
const proc=cp.spawn(bin,['serve','--port',String(port),'--host','127.0.0.1','--log-level','error'],{stdio:'ignore',env:{...process.env,WPT_CHROME_STATE_DIR:fs.mkdtempSync(path.join(os.tmpdir(),'qa-enter-'))}});let b;
try{const endpoint='http://127.0.0.1:'+port;let version;for(let i=0;i<100;i++){try{version=await(await fetch(endpoint+'/json/version')).json();break}catch{}await new Promise(r=>setTimeout(r,100))}
b=await pw.chromium.connectOverCDP(endpoint);const p=await b.newPage(),rows=[];
for(const cmd of ['insertOrderedList','insertUnorderedList'])for(const reverse of [false,true])for(const shape of ['same','other','next-same','next-other','partial','attr','sibling','locked']){
 const dst=cmd==='insertOrderedList'?'ol':'ul',opp=dst==='ol'?'ul':'ol',src=shape==='same'?dst:opp;
 await p.setContent('<div id=h contenteditable>'+(shape==='sibling'?'<b>left</b>':'')+'foo<'+src+(shape==='attr'?' style="color:red"':'')+'><li>bar</li>'+(shape==='partial'?'<li>more</li>':'')+'</'+src+'>'+(shape==='next-same'?'<'+dst+'><li>extra</li></'+dst+'>':shape==='next-other'?'<'+opp+'><li>extra</li></'+opp+'>':'')+(shape==='locked'?'<span contenteditable=false>locked</span>':'')+'baz</div>');
 rows.push(await p.evaluate(({cmd,shape,reverse})=>{const h=document.querySelector('#h'),s=getSelection(),outer=h.querySelector('ul,ol'),sub=outer.firstChild,w=document.createTreeWalker(h,NodeFilter.SHOW_TEXT),nodes={};let t;while(t=w.nextNode())nodes[t.data]=t;const first=nodes.foo,last=nodes.bar;
 h.focus();s.setBaseAndExtent(reverse?last:first,reverse?2:1,reverse?first:last,reverse?1:2);
 const point=n=>{const p=[];while(n&&n!==h){if(!n.parentNode)return null;p.unshift([...n.parentNode.childNodes].indexOf(n));n=n.parentNode}return n?p:null};
 const snap=()=>({html:h.innerHTML,state:document.queryCommandState(cmd),value:document.queryCommandValue(cmd),ol:document.queryCommandState('insertOrderedList'),ul:document.queryCommandState('insertUnorderedList'),anchor:[point(s.anchorNode),s.anchorOffset],focus:[point(s.focusNode),s.focusOffset],direction:s.direction,text:s.toString(),connected:[outer,sub,first,last].map(n=>n.isConnected)});
 const before=snap(),ok=document.execCommand(cmd),after=snap(),again=document.execCommand(cmd),twice=snap(),undo=document.execCommand('undo'),onceUndo=snap(),undo2=document.execCommand('undo'),restored=snap();return {cmd,reverse,shape,before,ok,after,again,twice,undo,onceUndo,undo2,restored};
 },{cmd,shape,reverse}));}
fs.writeFileSync(path.join(__dirname,'scripts-'+label+'.json'),JSON.stringify({version,rows},null,2));
}finally{if(b)await b.close();proc.kill()}})().catch(e=>{console.error(e);process.exitCode=1});
