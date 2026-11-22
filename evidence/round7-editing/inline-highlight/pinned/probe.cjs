const fs=require('fs'),path=require('path'),net=require('net'),cp=require('child_process'),os=require('os');
const cache=path.join(os.homedir(),'Library/Caches/lightningpanda'),pw=require(path.join(cache,'round6-reference-158/census/runner/node_modules/playwright-core'));
(async()=>{const [label,bin]=process.argv.slice(2),s=net.createServer();await new Promise(r=>s.listen(0,'127.0.0.1',r));const port=s.address().port;await new Promise(r=>s.close(r));
const proc=cp.spawn(bin,['serve','--port',String(port),'--host','127.0.0.1','--log-level','error'],{stdio:'ignore',env:{...process.env,WPT_CHROME_STATE_DIR:fs.mkdtempSync(path.join(os.tmpdir(),'qa-enter-'))}});let b;
try{const endpoint='http://127.0.0.1:'+port;let version;for(let i=0;i<100;i++){try{version=await(await fetch(endpoint+'/json/version')).json();break}catch{}await new Promise(r=>setTimeout(r,100))}
b=await pw.chromium.connectOverCDP(endpoint);const p=await b.newPage(),rows=[];
const fixtures=JSON.parse(fs.readFileSync(path.join(__dirname,'fixtures.json')));for(const f of fixtures){
 await p.setContent('<!doctype html><div id=h contenteditable></div>');await p.addScriptTag({path:path.join(cache,'wptci/wpt-full/editing/include/editor-test-utils.js')});
 rows.push(await p.evaluate(f=>{const h=document.querySelector('#h');new EditorTestUtils(h).setupEditingHost(f.html);const s=getSelection();
 const point=n=>{const a=[];while(n&&n!==h){if(!n.parentNode)return null;a.unshift([...n.parentNode.childNodes].indexOf(n));n=n.parentNode}return n?a:null;};
 const snap=()=>{const w=document.createTreeWalker(h,NodeFilter.SHOW_TEXT),glyphs=[];let n;while(n=w.nextNode()){let e=n.parentElement,bg='transparent';while(e){const v=getComputedStyle(e).backgroundColor;if(v!=='rgba(0, 0, 0, 0)'&&v!=='transparent'){bg=v;break}if(e===h)break;e=e.parentElement}for(const ch of n.data)glyphs.push([ch,bg,getComputedStyle(n.parentElement).fontSize])}return {html:h.innerHTML,glyphs,anchor:[point(s.anchorNode),s.anchorOffset],focus:[point(s.focusNode),s.focusOffset],direction:s.direction}};
 const before=snap();for(const [cmd,v] of f.commands)document.execCommand(cmd,false,v);return {name:f.name,url:f.url,before,after:snap()};},f));}
fs.writeFileSync(path.join(__dirname,'scripts-'+label+'.json'),JSON.stringify({version,rows},null,2));
}finally{if(b)await b.close();proc.kill()}})().catch(e=>{console.error(e);process.exitCode=1});
