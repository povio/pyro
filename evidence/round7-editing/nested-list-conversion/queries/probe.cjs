const fs=require('fs'),path=require('path'),net=require('net'),cp=require('child_process'),os=require('os');
const cache=path.join(os.homedir(),'Library/Caches/lightningpanda'),pw=require(path.join(cache,'round6-reference-158/census/runner/node_modules/playwright-core'));
(async()=>{const [label,bin]=process.argv.slice(2),s=net.createServer();await new Promise(r=>s.listen(0,'127.0.0.1',r));const port=s.address().port;await new Promise(r=>s.close(r));
const proc=cp.spawn(bin,['serve','--port',String(port),'--host','127.0.0.1','--log-level','error'],{stdio:'ignore',env:{...process.env,WPT_CHROME_STATE_DIR:fs.mkdtempSync(path.join(os.tmpdir(),'qa-enter-'))}});let b;
try{const endpoint='http://127.0.0.1:'+port;let version;for(let i=0;i<100;i++){try{version=await(await fetch(endpoint+'/json/version')).json();break}catch{}await new Promise(r=>setTimeout(r,100))}
b=await pw.chromium.connectOverCDP(endpoint);const p=await b.newPage(),rows=[];
for(const outer of ['ol','ul'])for(const inner of ['ol','ul'])for(const where of ['a','b','c','d','ac','bc','ad','bd','cd','caret-a','caret-b','caret-c','caret-d'])for(const reverse of [false,true]){
 await p.setContent('<div id=h contenteditable><'+outer+'><li>aaa</li><li>bbb<'+inner+'><li>ccc</li></'+inner+'></li><li>ddd</li></'+outer+'></div>');
 rows.push(await p.evaluate(({outer,inner,where,reverse})=>{const h=document.querySelector('#h'),s=getSelection(),t={},w=document.createTreeWalker(h,NodeFilter.SHOW_TEXT);let n;while(n=w.nextNode())t[n.data[0]]=n;h.focus();const key=where.replace('caret-',''),a=t[key[0]],b=t[key[key.length-1]],lo=1,hi=where.startsWith('caret-')?1:2;s.setBaseAndExtent(reverse?b:a,reverse?hi:lo,reverse?a:b,reverse?lo:hi);return {outer,inner,where,reverse,ol:document.queryCommandState('insertOrderedList'),ul:document.queryCommandState('insertUnorderedList')};},{outer,inner,where,reverse}));}
fs.writeFileSync(path.join(__dirname,'scripts-'+label+'.json'),JSON.stringify({version,rows},null,2));
}finally{if(b)await b.close();proc.kill()}})().catch(e=>{console.error(e);process.exitCode=1});
