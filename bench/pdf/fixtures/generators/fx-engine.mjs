const a=(i)=>`<a href="https://example.com/${i}">x</a>`;
const ln=(p,n)=>Array.from({length:n},(_,i)=>a(p+i)).join('<br>');
const box=(i,h,st='')=>`<a href="https://example.com/${i}" style="display:block;height:${h}px;${st}"></a>`;
export const html=`<!doctype html><body style="margin:0;font:16px/20px Arial"><div style="height:900px"></div><div>${ln('a',3)}</div><div style="height:10px"></div>${box('b',300,'break-inside:avoid')}<div style="height:600px"></div>${box('c',1200,'break-inside:avoid;margin-top:20px')}<div style="widows:3">${ln('d',4)}</div><div style="break-before:page;margin-top:30px">${ln('e',1)}</div><div style="width:900px">${ln('f',2)}</div>`;
export const cases=[{name:'mixed',html,opts:{paperWidth:8.5,paperHeight:11,marginTop:.4,marginBottom:.4,marginLeft:.4,marginRight:.4,scale:1}},{name:'A4',html,opts:{paperWidth:8.27,paperHeight:11.69,marginTop:0,marginBottom:0,marginLeft:0,marginRight:0,scale:1}}];
