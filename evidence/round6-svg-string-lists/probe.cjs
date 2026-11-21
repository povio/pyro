const fs=require('fs'), path=require('path'),cp=require('child_process'),net=require('net');
const pw=require('playwright-core');
const [label,bin]=process.argv.slice(2);
(async()=>{
 let browser,proc;
 try {
 if(bin==='chrome')browser=await pw.chromium.launch({executablePath:process.env.CHROME_BIN,headless:true});
 else {
 const server=net.createServer();await new Promise(r=>server.listen(0,'127.0.0.1',r));const port=server.address().port;await new Promise(r=>server.close(r));
 proc=cp.spawn(bin,['serve','--host','127.0.0.1','--port',String(port),'--log-level','error'],{stdio:'ignore'});
 const url='http://127.0.0.1:'+port;
 for(let i=0;i<100;i++){try{if((await fetch(url+'/json/version')).ok)break}catch{}await new Promise(r=>setTimeout(r,100))}
 browser=await pw.chromium.connectOverCDP(url)}
 const page=await browser.newPage();await page.goto('data:text/html,<html><body></body></html>');
 const result=await page.evaluate(()=>{
 const out={};document.body.innerHTML='<svg><g></g></svg>';const el=document.querySelector('g');
 for(const attr of ['systemLanguage','requiredExtensions']){
  const l=el[attr];if(!l){out[attr]=null;continue}
  const val=()=>[el.getAttribute(attr),l.length,Array.from({length:l.length},(_,i)=>l[i])];
  const tests={absent:val(),same:l===el[attr],proto:Object.getPrototypeOf(l).constructor.name};
  l.clear();tests.cleared=val();l.appendItem('en fr');tests.spaced=val();
  l.appendItem('de,zh');tests.comma=val();
  tests.removed=l.removeItem(0);tests.afterRemove=val();
  l[0]='new item';tests.replaced=val();try{l[3]='new';tests.outside='no throw'}catch(e){tests.outside=e.name}
  l.insertItemBefore('tail',9);tests.atEnd=val();
  el.setAttribute(attr,el.getAttribute(attr));tests.sameTextWrite=val();
  el.removeAttribute(attr);tests.removeAttribute=val();l.initialize('');tests.emptyItem=val();
  l.clear();tests.clearAgain=val();el.setAttribute(attr,'');tests.emptyAttr=val();
  for(const input of [' one  two, three four ','a\tb\nc','a,,b','  ','a,b,c',',','x, y,']){el.setAttribute(attr,input);tests[input]=val()}
  l.initialize('one');l.appendItem('two');tests.wrappedIndex=l.getItem(2**70);
  const order=[];const inserted=l.insertItemBefore({toString(){order.push('value');return 'three'}},{valueOf(){order.push('index');return 1}});
  tests.coercion=[inserted,order,val()];
  tests.keys=Object.keys(l);tests.iteration=typeof l[Symbol.iterator]==='function'?[...l]:null;
  const detached=el.cloneNode(true);const held=detached[attr];detached.setAttributeNS('urn:other',attr,'ignored');
  const before=held.getItem(0);detached.setAttributeNS(null,attr,'after');tests.namespace=[before,held.getItem(0)];
  tests.strong=[held===detached[attr],held instanceof SVGStringList];tests.arity=[l.getItem.length,l.initialize.length,l.replaceItem.length,l.clear.length];
  for(const [key,fn] of Object.entries({missing:()=>l.replaceItem('x'),neg:()=>l.removeItem(-1),symbol:()=>l.appendItem(Symbol('x'))})){try{fn();tests[key]='no throw'}catch(e){tests[key]=e.name}}
  const owner=document.implementation.createHTMLDocument('list');
  held.initialize('x y');owner.adoptNode(detached);
  tests.adoption=[held===detached[attr],held.length,held[0],detached.ownerDocument===owner];
  l.appendItem({toString(){el.setAttribute(attr,attr==='systemLanguage'?'red,green':'red green');return 'blue'}});
  tests.reentrant=val();const node=el.getAttributeNode(attr);node.value=attr==='systemLanguage'?'a, b':'a  b';tests.attrNodeWrite=val();
  out[attr]=tests;
 }
 return out;
 });
 fs.writeFileSync(path.resolve(__dirname,label+'.json'),JSON.stringify(result,null,2))
 }finally{if(browser)await browser.close();if(proc)proc.kill()}
})().catch(e=>{console.error(e);process.exit(1)});
