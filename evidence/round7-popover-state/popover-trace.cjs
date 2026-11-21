const fs=require('fs'),path=require('path'),net=require('net'),cp=require('child_process'),os=require('os');
const cache=path.dirname(__dirname),pw=require(path.join(cache,'round6-reference-158/census/runner/node_modules/playwright-core'));
(async()=>{const [label,bin]=process.argv.slice(2),s=net.createServer();await new Promise(r=>s.listen(0,'127.0.0.1',r));const port=s.address().port;await new Promise(r=>s.close(r));
const proc=cp.spawn(bin,['serve','--port',String(port),'--host','127.0.0.1','--log-level','error'],{stdio:'ignore',env:{...process.env,WPT_CHROME_STATE_DIR:fs.mkdtempSync(path.join(os.tmpdir(),'qa-enter-'))}});let b;
try{const endpoint='http://127.0.0.1:'+port;let version;for(let i=0;i<100;i++){try{version=await(await fetch(endpoint+'/json/version')).json();break}catch{}await new Promise(r=>setTimeout(r,100))}
b=await pw.chromium.connectOverCDP(endpoint);const p=await b.newPage(),rows=[];
for(const mode of ['remove','second-change']){
 await p.setContent('<div id=outer popover=auto><div id=child popover=auto></div></div>');
 rows.push(await p.evaluate(async mode=>{const outer=document.querySelector('#outer'),child=document.querySelector('#child'),events=[],states=[];
 const task=ms=>new Promise(r=>setTimeout(r,ms)),frame=()=>new Promise(r=>requestAnimationFrame(r));
 const snap=phase=>states.push({phase,parentOpen:outer.matches(':popover-open'),childOpen:child.matches(':popover-open'),childConnected:child.isConnected,attribute:child.popover,eventCount:events.length});
 outer.showPopover();child.showPopover();await task(100);for(const [id,el] of [['outer',outer],['child',child]])for(const t of ['beforetoggle','toggle'])el.addEventListener(t,e=>events.push([id,t,e.oldState,e.newState]));
 let once=false;child.addEventListener('beforetoggle',e=>{if(e.newState==='closed'&&!once){once=true;if(mode==='remove')child.remove();else child.popover='hint';}});
 snap('before');child.popover='manual';snap('immediate');await task(0);snap('next task');await Promise.race([frame().then(frame),task(500)]);snap('frame window');await task(250);snap('settled');return {mode,states,events};
 },mode));}
fs.writeFileSync(path.join(__dirname,'popover-trace-'+label+'.json'),JSON.stringify({version,rows},null,2))
}finally{if(b)await b.close();proc.kill()}})().catch(e=>{console.error(e);process.exitCode=1});
