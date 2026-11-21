const fs = require('node:fs');
const http = require('node:http');
const net = require('node:net');
const cp = require('node:child_process');
const path = require('node:path');
const cache = path.resolve(__dirname, '../..');
const pw = require(path.join(cache, 'round6-reference-158/census/runner/node_modules/playwright-core'));
const [label,binary] = process.argv.slice(2);
const listen = server => new Promise(resolve => server.listen(0,'127.0.0.1',() => resolve(server.address().port)));
(async () => {
 const httpd=http.createServer((req,res) => res.end('<!doctype html><title>Popover mutation</title><body></body>'));
 const port=await listen(httpd), socket=net.createServer(), cdpPort=await listen(socket); await new Promise(resolve=>socket.close(resolve));
 const proc=cp.spawn(binary,['serve','--port',String(cdpPort),'--host','127.0.0.1','--log-level','error'],{stdio:'ignore',
  env:{...process.env,WPT_CHROME_REFERENCE_MODE:'wpt-stable',WPT_CHROME_STATE_DIR:__dirname}});
 let browser;
 try {
  const cdp='http://127.0.0.1:'+cdpPort;
  for(let i=0;i<100;i++){try{if((await fetch(cdp+'/json/version')).ok)break;}catch{} await new Promise(resolve=>setTimeout(resolve,75));}
  browser=await pw.chromium.connectOverCDP(cdp);const page=await browser.newPage();await page.goto('http://127.0.0.1:'+port);
  const results=await page.evaluate(async()=>{
   const make=(root=document.body)=>{let e=document.createElement('div');e.popover='auto';root.append(e);return e;};
   const open=e=>e.matches(':popover-open');
   const flush=()=>new Promise(resolve=>setTimeout(resolve,50));
   const results={};
   for(const [key,change] of Object.entries({
    equivalent:e=>{e.popover='AuTo'}, namespaced:e=>{e.setAttributeNS('urn:demo','t:popover','manual')},
    liveAttr:e=>{e.getAttributeNode('popover').value='manual'},namedMap:e=>{e.attributes.removeNamedItem('popover')},
    replaceAttr:e=>{let a=document.createAttribute('popover');a.value='hint';e.setAttributeNode(a)},
    nullish:e=>{e.popover=null}, handlerRewrites:e=>{e.addEventListener('beforetoggle',()=>{e.popover='manual'},{once:true});e.popover='hint'}
   })){
    const e=make(),events=[];e.showPopover();await flush();
    for(const t of ['beforetoggle','toggle'])e.addEventListener(t,x=>events.push([t,x.oldState,x.newState]));
    change(e);const immediate={open:open(e),attr:e.getAttribute('popover'),reflected:e.popover};await flush();
    results[key]={immediate,events};e.remove();
   }
   let parent=make(),child=make(parent);parent.showPopover();child.showPopover();
   child.addEventListener('beforetoggle',()=>child.remove(),{once:true});child.hidePopover();
   results.detachedChild={parentOpen:open(parent),childConnected:child.isConnected};parent.remove();return results;
  });
  fs.writeFileSync(path.join(__dirname,label+'-observations.json'),JSON.stringify(results,null,2));
 }finally{if(browser)await browser.close();proc.kill();httpd.close();}
})().catch(e=>{console.error(e);process.exitCode=1})
