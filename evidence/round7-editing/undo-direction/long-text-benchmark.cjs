const fs=require('fs'), path=require('path'), os=require('os'), net=require('net'), cp=require('child_process');
const {chromium}=require('playwright-core');
async function main() {
  const [label,bin]=process.argv.slice(2);
  const server=net.createServer(); await new Promise(r=>server.listen(0,'127.0.0.1',r));
  const port=server.address().port; await new Promise(r=>server.close(r));
  const proc=cp.spawn(bin,['serve','--port',String(port),'--host','127.0.0.1','--log-level','error'],{stdio:'ignore',env:{...process.env,WPT_CHROME_STATE_DIR:path.join(__dirname,'chrome-state')}});
  let browser; try {
    let version;for(let i=0;i<150;i++){try{version=await(await fetch('http://127.0.0.1:'+port+'/json/version')).json();break}catch{}await new Promise(r=>setTimeout(r,100))}
    browser=await chromium.connectOverCDP('http://127.0.0.1:'+port); const rows=[];
    for (const units of [4096,32768,262144]) for(const cmd of ['insertText','delete']){
      const ctx=await browser.newContext();
      try{
        const p=await ctx.newPage();
        await p.setContent('<textarea></textarea>');
        const data=await p.evaluate(({units,cmd})=>{
          const h=document.querySelector('textarea'), source='ab😀'.repeat(units/4);
          h.value=source;h.focus();h.setSelectionRange(h.value.length,h.value.length,'forward');
          for(let i=0;i<8;i++)document.execCommand(cmd,false,cmd==='insertText'?'Z':undefined);
          h.value=source;h.setSelectionRange(h.value.length,h.value.length,'forward');
          const startUTC=new Date().toISOString();
          const count=64, t0=performance.now();let accepted=0;
          for(let i=0;i<count;i++) if(document.execCommand(cmd,false,cmd==='insertText'?'Z':undefined))accepted++;
          const elapsedMs=performance.now()-t0, endUTC=new Date().toISOString();
          const expected=cmd==='insertText'?source+'Z'.repeat(count):'ab😀'.repeat((units/4)-Math.floor(count/3)).slice(0,-2);
          const valid=accepted===count && h.value===expected && h.selectionStart===expected.length && h.selectionEnd===expected.length;
          return {elapsedMs,startUTC,endUTC,count,accepted,valid,unitsAfter:h.value.length};
        },{units,cmd});
        if(!data.valid)throw new Error(JSON.stringify({units,cmd,data}));
        rows.push({units,cmd,...data});
      } finally{await ctx.close()}
    }
    fs.writeFileSync(path.resolve('bench-'+label+'.json'),JSON.stringify({version,rows}));
  }finally {if(browser)await browser.close();proc.kill()}
}main().catch(e=>{console.error(e);process.exitCode=1});
