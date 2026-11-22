const {chromium} = require('/private/tmp/rc5-studio-pilot.vSYBbx/runner/node_modules/playwright-core');
(async()=>{
const native = process.argv[2] === 'chrome';
const browser = native ? await chromium.launch({executablePath:'/Applications/Google Chrome.app/Contents/MacOS/Google Chrome'}) : await chromium.connectOverCDP('http://127.0.0.1:48984',{timeout:5000});
try {
const page=await browser.newPage();
await page.goto((native?'file://'+__dirname:'http://fixture.test')+'/outer.html',{timeout:5000});
console.log(JSON.stringify(await page.evaluate(()=>Array.from(document.querySelectorAll('div'),row=>{
const box=node=>node.getBoundingClientRect();
const before=box(row.firstElementChild), after=box(row.lastElementChild);
const op=row.querySelector('mo');
return {outer:after.left-before.right,markers:[before.width,after.width],gaps:op?[box(op).left-box(op.previousElementSibling).right,box(op.nextElementSibling).left-box(op).right]:null};
})),null,2));
}finally{await browser.close()}
})().catch(error=>{console.error(error);process.exitCode=1});
