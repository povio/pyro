const fs=require('fs'), path=require('path'),cp=require('child_process'),net=require('net');
const pw=require('playwright-core');
const [label,bin]=process.argv.slice(2);
(async()=>{
 let browser,proc;
 try {
 if(bin==='chrome')browser=await pw.chromium.launch({executablePath:'/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',headless:true});
 else {
 const server=net.createServer();await new Promise(r=>server.listen(0,'127.0.0.1',r));const port=server.address().port;await new Promise(r=>server.close(r));
 proc=cp.spawn(bin,['serve','--host','127.0.0.1','--port',String(port),'--log-level','error'],{stdio:'ignore'});
 const url='http://127.0.0.1:'+port;
 for(let i=0;i<100;i++){try{if((await fetch(url+'/json/version')).ok)break}catch{}await new Promise(r=>setTimeout(r,100))}
 browser=await pw.chromium.connectOverCDP(url)}
 const page=await browser.newPage();await page.goto('data:text/html,<html><body></body></html>');
 const result=await page.evaluate(()=>{
 document.body.innerHTML='<style>#styled {width:0px;height:30px} #fromCss {d:path("M2 1 L12 1")}</style><svg width="200" height="100" viewBox="0 0 200 100"><rect id="rounded" x="4" y="7" width="50" height="40" rx="8" ry="3"/><ellipse id="tall" cx="1" cy="2" rx="6" ry="14"/><ellipse id="wide" cx="1" cy="2" rx="14" ry="6"/><rect id="styled" width="20" height="20"/><line id="pct" x1="10%" x2="50%" y1="0" y2="0"/><path id="fromCss" d="M0 0 L80 5"/><path id="curve" d="M0 0 Q 10 20 20 0"/><path id="multi" d="M0 0 L2 0 M10 3 L20 3"/></svg>';
 const probes={};for(const id of ['rounded','tall','wide','styled','pct','fromCss','curve','multi']){
 const el=document.getElementById(id);const len=el.getTotalLength();const pts=[];
 for(const at of [0,len/4,len]){try{const p=el.getPointAtLength(at);pts.push([p.x,p.y,p.z,p.w,p instanceof DOMPoint])}catch(e){pts.push(e.name)}}
 probes[id]={len,pts}}
 const pt=document.getElementById('curve').getPointAtLength(0);
 let mutation;try{pt.x=99;mutation=pt.x}catch(e){mutation=e.name}
 probes.pointType={ctor:pt.constructor.name,svg:typeof SVGPoint!=='undefined'&&pt instanceof SVGPoint,mutation,transform:typeof pt.matrixTransform,create:typeof document.querySelector('svg').createSVGPoint};
 const svg=document.querySelector('svg');svg.removeAttribute('viewBox');svg.style.width='400px';
 probes.cssViewport=document.getElementById('pct').getTotalLength();
 for(const [tag,attrs] of [['rect',{width:0,height:0}],['rect',{width:0,height:10}],['rect',{width:10,height:0}],['ellipse',{rx:0,ry:10}],['ellipse',{rx:10}]]) {
  const node=document.createElementNS(svg.namespaceURI,tag);for(const [k,v] of Object.entries(attrs))node.setAttribute(k,v);svg.append(node);
  let pt;try{const p=node.getPointAtLength(0);pt=[p.x,p.y]}catch(e){pt=e.name}
  probes[tag+JSON.stringify(attrs)]={len:node.getTotalLength(),pt};
 }
 const wrapper=document.createElement('div');wrapper.style.width='200px';document.body.append(wrapper);
 wrapper.innerHTML='<svg width="50%" height="100"><line x2="50%"/></svg>';
 const outer=wrapper.firstElementChild;probes.rootPercent={box:outer.getBoundingClientRect().width,len:outer.firstElementChild.getTotalLength()};
 return probes
 });
 fs.writeFileSync(path.resolve(__dirname,label+'.json'),JSON.stringify(result,null,2))
 }finally{if(browser)await browser.close();if(proc)proc.kill()}
})().catch(e=>{console.error(e);process.exit(1)});
