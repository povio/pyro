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
   const inputs=['1-2','1px-2px','1,2,','10px 25% BAD','1/**/2','1 2 ',' 1, 2','1e-2px,+2.5%','1ch 2cap','1ic','1lh','1vmin 1q','1Q','0x1','none'];
   out.inputs=inputs.map(str=>{text.setAttribute('x',str);return [str,Array.from(text.x.baseVal,x=>[x.valueAsString,x.unitType])];});
   text.setAttribute('x','1px 2px');const list=text.x.baseVal,anim=text.x.animVal;const detached=anim.getItem(1);
   text.setAttribute('x','1px');const error=fn=>{try{fn();return 'ok'}catch(e){return e.name}};
   out.detached=[detached.valueAsString,error(()=>detached.value=3)];
   const attached=list.appendItem(detached);out.copy=[attached===detached,attached.valueAsString];
   const prev=list.getItem(0);const initialized=list.initialize(prev);out.initialize=[prev===initialized,initialized===list.getItem(0),prev.valueAsString,list.length];
   return out;
 });
 fs.writeFileSync(path.join(w,label+'-parse-probe.json'),JSON.stringify(values,null,2));
 }finally{if(b)await b.close();if(proc)proc.kill();server.close()}})().catch(e=>{console.error(e);process.exitCode=1});
