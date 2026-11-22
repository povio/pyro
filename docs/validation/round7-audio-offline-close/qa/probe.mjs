import {spawn,execFileSync} from 'node:child_process';
import {mkdtempSync,readFileSync,writeFileSync,rmSync,existsSync} from 'node:fs';
import {createHash} from 'node:crypto';
import {fileURLToPath} from 'node:url';
process.chdir(fileURLToPath(new URL('.',import.meta.url)));
const app='/Applications/Google Chrome.app', executable=app+'/Contents/MacOS/Google Chrome';
execFileSync('codesign',['--verify','--deep','-R','=identifier "com.google.Chrome" and anchor apple generic and certificate leaf[subject.OU] = "EQHXZ8M8AV"',app]);
const source=readFileSync(process.argv[2]??'/private/tmp/pyro-r7-param/crates/pyro-webapi/src/webaudio.js','utf8');
const digest=createHash('sha256').update(source).digest('hex');
if(!existsSync(digest+'.js'))writeFileSync(digest+'.js',source);
async function controls(Context){
  const rate=32768, make=(finite=false)=>new Context({numberOfChannels:2,sampleRate:rate,...(finite?{length:517}:{})});
  const result={},frame=context=>Math.round(context.currentTime*rate);
  const outcome=async fn=>{try{await fn();return 'fulfilled'}catch(error){return error.name}};
  const pattern=(channel,index)=>(channel?-1:1)*(1+(index%251))/256;
  function wire(context){const buffer=context.createBuffer(2,1024,rate);for(let channel=0;channel<2;channel++)for(let index=0;index<1024;index++)buffer.getChannelData(channel)[index]=pattern(channel,index);const node=context.createBufferSource();node.buffer=buffer;node.connect(context.destination);node.start()}
  function inspect(buffer,start){let bad=0;for(let channel=0;channel<buffer.numberOfChannels;channel++)for(let index=0;index<buffer.length;index++)if(buffer.getChannelData(channel)[index]!==pattern(channel,start+index))bad++;return [buffer.length,buffer.numberOfChannels,bad]}
  for(const [key,run] of Object.entries({
    positionalNull:()=>{const context=new Context(2,null,rate);return {length:context.length}},
    indefiniteQueue:async()=>{const context=make();wire(context);const order=[];const chunks=await Promise.all([[],[null],[129]].map((args,index)=>context.startRendering(...args).then(buffer=>{order.push(index);return buffer})));let start=0;const stereo=chunks.map(buffer=>{const row=inspect(buffer,start);start+=buffer.length;return row});return {length:context.length,stereo,order,frame:frame(context),before:context.state,close:await outcome(()=>context.close()),after:context.state,renderAfter:await outcome(()=>context.startRendering()),closeAgain:await outcome(()=>context.close())}},
    activeCloseTiming:async()=>{const context=make();await context.startRendering();const close=context.close();const immediate=context.state;await Promise.resolve();const microtask=context.state;await close;return [immediate,microtask,context.state]},
    finiteFresh:async()=>{const context=make(true);const close=outcome(()=>context.close());const immediate=context.state;return [immediate,await close,context.state,await outcome(()=>context.startRendering())]},
    finiteEnd:async()=>{const context=make(true);wire(context);const buffer=await context.startRendering();return [inspect(buffer,0),context.state,await outcome(()=>context.close())]},
    suspension:async()=>{const context=make();wire(context);const paused=context.suspend(128/rate);const chunk=context.startRendering(384);await paused;const checkpoint=[context.state,frame(context)];await context.resume();const buffer=await chunk;return [checkpoint,inspect(buffer,0),frame(context),await outcome(()=>context.close())]},
    finitePausedClose:async()=>{const context=make(true);const paused=context.suspend(128/rate);const first=outcome(()=>context.startRendering(384)),second=outcome(()=>context.startRendering());await paused;const checkpoint=[context.state,frame(context)];const close=await outcome(()=>context.close());const after=context.state;if(after==='suspended')await context.resume();return [checkpoint,close,after,await first,await second,context.state]}
  }))try{result[key]=await Promise.race([Promise.resolve().then(run),new Promise((_,reject)=>setTimeout(()=>reject(Error('deadline')),2500))])}catch(error){result[key]={error:error.name,message:error.message}}
  return result;
}
for(const mode of ['native','candidate']){
  const profile=mkdtempSync('./profile-');const flags=['--headless=new','--no-first-run','--disable-default-apps','--disable-background-networking','--remote-debugging-pipe','--enable-blink-features=OfflineAudioContextIncrementalRendering','--user-data-dir='+process.cwd()+'/'+profile,'about:blank'];const child=spawn(executable,flags,{stdio:['ignore','ignore','ignore','pipe','pipe']});const exit=new Promise(resolve=>child.once('exit',resolve));let counter=0,text='';const pending=new Map();child.stdio[4].setEncoding('utf8').on('data',piece=>{text+=piece;let end;while((end=text.indexOf('\0'))>=0){const msg=JSON.parse(text.slice(0,end));text=text.slice(end+1);pending.get(msg.id)?.(msg)}});
  function call(method,params={},sessionId){return new Promise((resolve,reject)=>{const id=++counter;const timer=setTimeout(()=>reject(Error(method+' deadline')),20000);pending.set(id,msg=>{clearTimeout(timer);pending.delete(id);msg.error?reject(Error(JSON.stringify(msg.error))):resolve(msg.result)});child.stdio[3].write(JSON.stringify({id,method,params,sessionId})+'\0')})}
  try{const version=await call('Browser.getVersion');const {targetInfos}=await call('Target.getTargets');const {sessionId}=await call('Target.attachToTarget',{targetId:targetInfos.find(target=>target.type==='page').targetId,flatten:true});const constructor=mode==='native'?'OfflineAudioContext':`(${source.trim()})({timer:setTimeout,clearTimer:clearTimeout,activated:()=>true}).OfflineAudioContext`;const response=await call('Runtime.evaluate',{expression:`(${controls})(${constructor})`,awaitPromise:true,returnByValue:true},sessionId);if(response.exceptionDetails)throw Error(JSON.stringify(response.exceptionDetails));writeFileSync(mode+'.json',JSON.stringify({mode,version,flags:flags.filter(flag=>!flag.startsWith('--user-data-dir')),sourceSHA256:mode==='candidate'?digest:null,results:response.result.value},null,2)+'\n')}finally{child.kill('SIGKILL');await exit;rmSync(profile,{recursive:true,force:true})}
}
