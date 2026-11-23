const fs=require('fs'),path=require('path'),net=require('net'),cp=require('child_process'),os=require('os');
const cache=path.join(os.homedir(),'Library/Caches/lightningpanda'),pw=require(path.join(cache,'round6-reference-158/census/runner/node_modules/playwright-core'));
(async()=>{const [label,bin]=process.argv.slice(2),s=net.createServer();await new Promise(r=>s.listen(0,'127.0.0.1',r));const port=s.address().port;await new Promise(r=>s.close(r));
const proc=cp.spawn(bin,['serve','--port',String(port),'--host','127.0.0.1','--log-level','error'],{stdio:'ignore',env:{...process.env,WPT_CHROME_STATE_DIR:fs.mkdtempSync(path.join(os.tmpdir(),'qa-enter-'))}});let b;
try{const endpoint='http://127.0.0.1:'+port;let version;for(let i=0;i<100;i++){try{version=await(await fetch(endpoint+'/json/version')).json();break}catch{}await new Promise(r=>setTimeout(r,100))}
b=await pw.chromium.connectOverCDP(endpoint);const p=await b.newPage(),rows=[];
const wpt=path.join(cache,'wptci/wpt-full/editing');const rules=fs.readFileSync(path.join(wpt,'include/reset.css'),'utf8');const data=require('vm').runInNewContext(fs.readFileSync(path.join(wpt,'data/bold.js'),'utf8')+';browserTests');const cases=JSON.parse(fs.readFileSync(path.join(__dirname,'exact-targets.json'),'utf8'));
for(const x of cases)for(const reset of [false,true]){
 const test=data.find(a=>a[0]===x.input && JSON.stringify(a[1])===JSON.stringify(x.commands));if(!test)throw Error(x.name);const expected=test[2];if(typeof expected!=='string')throw Error('unexpected alternative '+x.name);
 await p.setContent('<style>'+(reset?rules:'')+'</style><div id=h contenteditable></div>');rows.push(await p.evaluate(({x,expected,reset})=>{
 const h=document.querySelector('#h'),strip=v=>v.replace(/[{}\[\]]/g,''),input=x.input.replace(/<[^>]*>/g,''),lo=input.search(/[\[{]/),hi=input.search(/[\]}]/)-1;
 const glyphs=()=>{const w=document.createTreeWalker(h,NodeFilter.SHOW_TEXT);let n,g=[];while(n=w.nextNode())for(const ch of n.data){const s=getComputedStyle(n.parentElement);g.push([ch,s.fontWeight,s.color])}return g};
 h.innerHTML=strip(expected);const pinned=glyphs();h.innerHTML=strip(x.input);h.focus();const points=[];const w=document.createTreeWalker(h,NodeFilter.SHOW_TEXT);let n,offset=0;while(n=w.nextNode()){points.push([n,offset,offset+n.length]);offset+=n.length}
 const a=points.find(v=>lo>=v[1]&&lo<v[2]),b=points.find(v=>hi>v[1]&&hi<=v[2]);getSelection().setBaseAndExtent(a[0],lo-a[1],b[0],hi-b[1]);for(const [name,value] of x.commands)document.execCommand(name,false,value);
 return {...x,reset,lo,hi,expected:strip(expected),pinned,commandHTML:h.innerHTML,command:glyphs()};},{x,expected,reset}));}
fs.writeFileSync(path.join(__dirname,'exact-'+label+'.json'),JSON.stringify({version,rows},null,2));
}finally{if(b)await b.close();proc.kill()}})().catch(e=>{console.error(e);process.exitCode=1});
