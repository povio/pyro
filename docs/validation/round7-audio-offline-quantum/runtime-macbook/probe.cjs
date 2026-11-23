const fs=require('node:fs'), http=require('node:http'), net=require('node:net'), cp=require('node:child_process'), path=require('node:path');
const cache=path.dirname(__dirname), root=cache+'/wptci/wpt-full', ref=cache+'/round6-reference-158/census';
const pw=require(ref+'/runner/node_modules/playwright-core');
const label=process.argv[2], dir='webaudio/the-audio-api/the-offlineaudiocontext-interface/';
const cases=['offlineaudiocontext-rendersizehint.html','offlineaudiocontext-suspend-rendersizehint.https.html','offlineaudiocontext-close.html'];
const server=http.createServer((req,res)=>{
 const p=new URL(req.url,'http://localhost').pathname;
 if(p==='/'){res.end('<!doctype html><body>');return}
 try{let body=fs.readFileSync(root+p,'utf8');if(p==='/resources/testharness.js')body+='\nadd_completion_callback((tests,s)=>globalThis.qaRows={status:s.status,tests:tests.map(t=>[t.name,t.status,t.message])});';
 res.setHeader('content-type',p.endsWith('.js')?'application/javascript':'text/html');res.end(body)}catch{res.writeHead(404);res.end()}
});
const listen=s=>new Promise(r=>s.listen(0,'127.0.0.1',()=>r(s.address().port)));
(async()=>{const port=await listen(server),sock=net.createServer(),cdp=await listen(sock);await new Promise(r=>sock.close(r));
 const bin=label==='reference'?ref+'/launch-158':__dirname+'/'+label;
 const proc=cp.spawn(bin,['serve','--host','127.0.0.1','--port',String(cdp),'--log-level','error'],{stdio:['ignore',fs.openSync(label+'.stdout','w'),fs.openSync(label+'.stderr','w')],env:{...process.env,WPT_CHROME_STATE_DIR:__dirname+'/ref-state'}});
 let browser;const out={};try{
 for(let i=0;i<100;i++){if(proc.exitCode!=null)throw Error('exit '+proc.exitCode);try{if((await fetch('http://127.0.0.1:'+cdp+'/json/version')).ok)break}catch{}await new Promise(r=>setTimeout(r,100))}
 browser=await pw.chromium.connectOverCDP('http://127.0.0.1:'+cdp);const page=await browser.newPage();page.setDefaultTimeout(10000);
 const url='http://127.0.0.1:'+port+'/';await page.goto(url);
 out.pcm=await page.evaluate(async()=>{
 const rows=[];for(const quantum of [128,13,441,33001]){
 const length=quantum+829,c=new OfflineAudioContext({numberOfChannels:2,length,sampleRate:48000,renderSizeHint:quantum});
 const s=c.createBufferSource(),x=c.createBuffer(2,length,48000),h=c.createBuffer(1,513,48000);
 const taps=[[0,.5],[127,-.25],[128,.125],[512,.75]], impulses=[[0,.25],[quantum-1,-.5],[quantum,.125],[length-1,.5]];
 for(const [i,v] of taps)h.getChannelData(0)[i]=v;
 for(let ch=0;ch<2;ch++)for(const [i,v] of impulses)x.getChannelData(ch)[i]+=v*(ch+1);
 const n=c.createConvolver();n.normalize=false;n.buffer=h;s.buffer=x;s.connect(n).connect(c.destination);s.start();
 const b=await c.startRendering();const channels=[];
 for(let ch=0;ch<2;ch++){const exp=new Float32Array(length);for(const [i,v] of impulses)for(const [j,w] of taps)if(i+j<length)exp[i+j]+=v*w*(ch+1);
 const actual=b.getChannelData(ch);let bad=0,worst=0;for(let i=0;i<length;i++){const d=Math.abs(actual[i]-exp[i]);if(!(d<=1e-5))bad++;worst=Math.max(worst,d)}channels.push({bad,worst,length:actual.length,tail:actual[length-1]})}
 rows.push({quantum,actualQuantum:c.renderQuantumSize,channels})}return rows});
 for(const file of cases){try{await page.goto(url+dir+file);await page.waitForFunction(()=>!!globalThis.qaRows);out[file]=await page.evaluate(()=>qaRows)}catch(e){out[file]={error:String(e)}}}
 }finally{fs.writeFileSync(label+'.json',JSON.stringify(out,null,2));if(browser)await browser.close();proc.kill();server.close()}
})().catch(e=>{console.error(e);process.exitCode=1});
