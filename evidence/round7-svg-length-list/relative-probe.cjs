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
   const out={surface:[typeof SVGLengthList,typeof SVGAnimatedLengthList]};
   if(typeof SVGLengthList!=="function")return out;
   const NS="http://www.w3.org/2000/svg";
   const el=(name,parent)=>parent.appendChild(document.createElementNS(NS,name));
   const svg=el("svg",document.body);svg.setAttribute("width","200");svg.setAttribute("height","100");
   const text=el("text",svg),span=el("tspan",text);text.setAttribute("font-size","10");
   const ref=document.createElement("div");document.body.appendChild(ref);
   const round=x=>Math.round(x*100)/100;
   const close=(x,y)=>Math.abs(x-y)<=0.15;
   for(const unit of ['rem','rex','rch','rcap','ric','rlh','vi','vb','svw','lvh','dvi','svmax','cqw','cqh','cqmin']){
     const result=[];out[unit]=result; text.setAttribute('x','10'+unit);ref.style.width='10'+unit;
     for(const [size,height] of [[20,32],[30,48]]){
       for (const el of [text,ref]) {el.style.fontFamily='serif';el.style.fontSize=size+'px';el.style.lineHeight=height+'px'}
       const x=text.x.baseVal[0],css=ref.getBoundingClientRect().width;
       if(!x){result.push({missing:true});continue}
       result.push({specified:x.valueInSpecifiedUnits,unit:x.unitType,svg:round(x.value),css:round(css),match:close(x.value,css)});
     }
   }
   return out;
 });
 fs.writeFileSync(path.join(w,label+'-relative-probe.json'),JSON.stringify(values,null,2));
 }finally{if(b)await b.close();if(proc)proc.kill();server.close()}})().catch(e=>{console.error(e);process.exitCode=1});
