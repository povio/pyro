const words = Array.from({length:1200},(_,i)=>'word'+i).join(' ');
const FIX = {
 links: `<!doctype html><body style="margin:0;font:20px/30px Arial"><div style="height:40px"></div><p style="margin:0 0 0 40px"><a href="https://a.test/1">example</a> text <a href="https://a.test/2">more</a></p><a href="https://a.test/3" style="display:block;margin-left:40px;width:200px;height:50px"></a>`,
 para: `<!doctype html><body style="margin:0;font:16px/20px Arial"><div style="width:400px">${words}</div>`,
 breaks: `<!doctype html><body style="margin:8px;font:16px/20px Arial"><h1>One</h1><div style="width:400px">${words.slice(0,4000)}</div><h2 style="break-before:page">Two</h2><p>after</p><div style="height:300px;background:#eee;break-inside:avoid">box</div><div style="height:900px;break-inside:avoid">tall</div><p style="break-after:page">end</p><p>last</p>`,
 wide: `<!doctype html><body style="margin:0;font:16px/20px Arial"><div style="width:500px">${words}</div>`,
 wide2: `<!doctype html><body style="margin:0;font:16px/20px Arial"><div style="width:1200px">${words}</div>`,
};
export const cases=[];
for (const [n,html] of Object.entries(FIX)) for (const [pn,w,h] of [['Letter',8.5,11],['A4',8.27,11.69]]) for (const m of [.4,1/2.54,.5,0]) for (const s of [1,.8,1.5])
  cases.push({name:[n,pn,m.toFixed(3),s].join('|'),html,opts:{paperWidth:w,paperHeight:h,marginTop:m,marginBottom:m,marginLeft:m,marginRight:m,scale:s}});
