const {chromium}=require('/private/tmp/pyro-r7-ordinary-audio/bench/node_modules/playwright-core');
const fs=require('node:fs'), crypto=require('node:crypto'), probe=require('./probe.cjs');
(async()=>{
  const browser=await chromium.launch({executablePath:'/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',chromiumSandbox:true,args:['--mute-audio']});
  try {
    for(const mode of process.argv.length > 2 ? [process.argv[2]] : ['native','base','candidate']) {
      const page=await browser.newPage(), source=mode==='native'?null:fs.readFileSync(process.argv[3] || mode+'.js','utf8');
      const results=await page.evaluate(probe,source);
      fs.writeFileSync(mode+'.json',JSON.stringify({mode,version:browser.version(),utc:new Date().toISOString(),sha256:source&&crypto.createHash('sha256').update(source).digest('hex'),results},null,2));
      await page.close();
    }
  }finally{await browser.close();}
})().catch(error=>{console.error(error);process.exitCode=1});
