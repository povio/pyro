const http=require('node:http'),fs=require('node:fs'),net=require('node:net'),path=require('node:path'),cp=require('node:child_process'),os=require('node:os');
const w=__dirname,ref=path.join(path.dirname(w),'round6-reference-158/census'),pw=require(path.join(ref,'runner/node_modules/playwright-core'));
const [label,explicit]=process.argv.slice(2);
const server=http.createServer((req,res)=>{res.setHeader('Content-Type','text/html');res.end('<!doctype html><body style="margin:0">');});
const listen=s=>new Promise(r=>s.listen(0,'127.0.0.1',()=>r(s.address().port)));
(async()=>{const port=await listen(server),sock=net.createServer(),cdp=await listen(sock);await new Promise(r=>sock.close(r));let proc,b;
 try{const bin=explicit||path.join(ref,'launch-158');
 const tmp=fs.mkdtempSync(path.join(os.tmpdir(),'r7-svg-fixture-'));
 proc=cp.spawn(bin,['serve','--host','127.0.0.1','--port',String(cdp),'--log-level','error','--load-resources','stylesheet','--load-resources','iframe'],{stdio:'ignore',env:{...process.env,WPT_CHROME_STATE_DIR:tmp,WPT_CHROME_REFERENCE_MODE:'wpt-stable'}});
 for(let i=0;i<120;i++){try{if((await fetch('http://127.0.0.1:'+cdp+'/json/version')).ok)break}catch{}await new Promise(r=>setTimeout(r,100))}
 b=await pw.chromium.connectOverCDP('http://127.0.0.1:'+cdp);const page=await b.newPage();await page.goto('http://127.0.0.1:'+port+'/page');
 const values=await page.evaluate(()=>{
 const make=name=>document.createElementNS('http://www.w3.org/2000/svg', name);
 const rows=[],svg=make('svg');
 for(const [tag,attr] of [['text','rotate'],['tspan','rotate'],['feColorMatrix','values'],['feConvolveMatrix','kernelMatrix'],...['R','G','B','A'].map(c=>['feFunc'+c,'tableValues'])]){
   const e=make(tag), wrapped=e[attr];
   const row={tag,attr,supported:!!wrapped};rows.push(row);
   if(!wrapped)continue;
   const b=wrapped.baseVal,a=wrapped.animVal;
   row.brands=[wrapped.constructor.name,b.constructor.name];
   row.identity=[e[attr]===wrapped,wrapped.baseVal===b,wrapped.animVal===a,b!==a];
   e.setAttribute(attr,'1, 2e1 -3.5');
   const item=b[0],shared=a[0],numbers=l=>Array.from(l,n=>n.value);
   row.initial=[numbers(b),numbers(a),item===b.getItem(0),shared!==item];
   e.setAttribute(attr,'4,5');
   row.sync=[item.value,shared.value,b[0]===item,a[0]===shared];
   const detached=svg.createSVGNumber();detached.value=7;
   const inserted=b.appendItem(detached),copy=b.insertItemBefore(item,0);
   row.insertion=[inserted===detached,copy!==item,item===b[1],numbers(b)];
   b[0]=detached;
   row.indexed=[b[0]!==detached,b[0].value,numbers(a)];
   const removed=b.removeItem(1);removed.value=9;
   row.removal=[removed===item,numbers(b),numbers(a)];
   const errors={};
   for(const [name,fn] of Object.entries({readonly:()=>a.clear(),readonlyItem:()=>{a[0].value=4},badArg:()=>b.appendItem({}),badIndex:()=>b.getItem(-1),nan:()=>{detached.value=NaN}})) {
     try{fn();errors[name]=null}catch(e){errors[name]=e.name}
   }
   row.errors=errors;
   const sibling=make(tag),other=sibling[attr].baseVal;
   other.clear();row.clearAbsent=sibling.getAttribute(attr);
   const fromOther=other.appendItem(svg.createSVGNumber());fromOther.value=11;
   const joined=b.appendItem(fromOther);joined.value=12;
   row.crossList=[joined!==fromOther,other[0]===fromOther,fromOther.value,joined.value];
   other.removeItem(0);row.removeLast=sibling.getAttribute(attr);

   e.setAttributeNS('urn:other','q:'+attr,'99');row.namespace=numbers(b);
   row.parses=[];
   for(const value of ['10 20','1e-2,+3.5','1,2x','1,,2','1,','1-2','1.2.3','1\f2','1e999','1/**/2','2px','3%','']){
     e.setAttribute(attr,value);row.parses.push([value,numbers(b)]);
   }
   b.initialize(detached);b.clear();row.cleared=[b.length,a.length,e.getAttribute(attr)];
 }
 return rows;});
 fs.writeFileSync(path.join(w,label+'-probe.json'),JSON.stringify(values,null,2));
 }finally{if(b)await b.close();if(proc)proc.kill();server.close()}})().catch(e=>{console.error(e);process.exitCode=1});
