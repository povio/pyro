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
 const values=await page.evaluate(async()=>{
   const t=document.createElement('textarea');t.value='abc';document.body.appendChild(t);
   const out={present:typeof t.createValueRange==='function'};if(!out.present)return out;
   const bad=fn=>{try{fn();return 'ok'}catch(e){return e.name}};
   const pair=r=>[r.startOffset,r.endOffset];const r=t.createValueRange(1,2);
   const offset=Object.getOwnPropertyDescriptor(AbstractRange.prototype,'startOffset').get;
   const container=Object.getOwnPropertyDescriptor(NodeRange.prototype,'startContainer').get;
   out.descriptors=[t.createValueRange.length,OpaqueRange.prototype.disconnect.length,
     'startContainer' in r,'endContainer' in r,r instanceof NodeRange,offset.call(r),
     document.createRange() instanceof NodeRange];
   out.brand=[bad(()=>container.call(r)),bad(()=>offset.call({})),bad(()=>r.disconnect.call({})),
     bad(()=>HTMLInputElement.prototype.createValueRange.call(t,0,0)),bad(()=>t.createValueRange()),
     bad(()=>t.createValueRange(0)),bad(()=>t.createValueRange(Symbol(),0)),bad(()=>t.createValueRange(0,1n)),
     bad(()=>new NodeRange()),bad(()=>new OpaqueRange())];
   out.conversions=[pair(t.createValueRange(2**64,2**32+2)),
     pair(t.createValueRange(NaN,Infinity)),pair(t.createValueRange(1.75,2.75)),
     pair(t.createValueRange(-(2**32),1)),bad(()=>t.createValueRange(-1,0))];
   const input=document.createElement('input');input.value='abc';
   const order=[];
   out.changedType=bad(()=>input.createValueRange({valueOf(){order.push('start');input.type='number';return 0}},
     {valueOf(){order.push('end');return 0}}));
   input.type='text';input.value='ab';
   out.changedValue=pair(input.createValueRange({valueOf(){input.value='a🌱b';return 1}},4));
   out.abrupt=bad(()=>input.createValueRange({valueOf(){throw new RangeError()}},{valueOf(){order.push('afterThrow');return 0}}));
   out.order=order;
   return out;
 });
 fs.writeFileSync(path.join(w,label+'-webidl.json'),JSON.stringify(values,null,2));
 }finally{if(b)await b.close();if(proc)proc.kill();server.close()}})().catch(e=>{console.error(e);process.exitCode=1});
