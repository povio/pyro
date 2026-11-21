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
 document.body.innerHTML='<svg><polygon points="1,2 3,4 5,6"></polygon><polyline></polyline></svg>';
 const svg=document.querySelector('svg'), el=svg.firstElementChild,other=svg.lastElementChild;
 const list=el.points,anim=el.animatedPoints,second=other.points;
 if(!list)return {missing:true};
 const out={},val=p=>[p.x,p.y],snap=l=>Array.from({length:l.length},(_,i)=>val(l.getItem(i))),err=fn=>{try{fn();return null}catch(e){return e.name}};
 const make=(x,y)=>{const p=svg.createSVGPoint();p.x=x;p.y=y;return p};
 const first=list[0],mid=list[1],last=list[2],am=anim[1],al=anim[2];
 out.initial=[list===el.points,anim===el.animatedPoints,list!==anim,first===list.getItem(0),snap(anim),err(()=>am.x=7),Object.keys(list)];
 const inserted=make(7,8);out.insert=[list.insertItemBefore(inserted,0)===inserted,list[0]===inserted,list[1]===first,el.getAttribute('points')];
 out.animAfterInsert=[val(am),val(al),snap(anim),err(()=>anim.clear())];
 const clone=second.appendItem(mid);out.copy=[clone!==mid,second[0]===clone,val(clone)];clone.x=11;out.copyIsolation=[val(mid),other.getAttribute('points')];
 const removed=list.removeItem(2);out.remove=[removed===mid,val(removed)];out.move=[second.appendItem(removed)===removed,second[1]===removed];
 el.setAttribute('points','12,13 14,15');out.external=[val(list[0]),val(first),val(last),val(am),val(al),snap(anim)];
 el.setAttribute('points','20,21 22,23 24,25');out.grow=[val(last),last===list[2]];
 out.readonly=[err(()=>anim[0].x=9),err(()=>anim.appendItem(make(1,2))),err(()=>anim[0]=make(1,2))];
 const before=list[0];const replacement=make(29,30);out.replace=[list.replaceItem(replacement,0)===replacement,before===list[0],val(before)];
 list.initialize(make(5,7));list.appendItem(make(8,11));out.geometry=[el.getTotalLength(),el.getBBox().width,el.getBBox().height,el.getAttribute('points')];
 list[1].x=11;out.edited=[el.getTotalLength(),el.getAttribute('points')];
 const keep=list[1],initialized=list.initialize(keep);keep.y=15;
 out.initializeHeld=[initialized===keep,keep===list[0],el.getAttribute('points')];
 out.validation=[err(()=>list.getItem(-1)),err(()=>list.appendItem({x:1,y:2})),err(()=>list.replaceItem(make(1,2))),err(()=>list.getItem(2**70))];
 list.clear();out.cleared=[el.getAttribute('points'),list.length,anim.length];
 const old=other.points[0];document.implementation.createHTMLDocument('points').adoptNode(other);out.adopt=[val(old),old===other.points[0]];
 const namespaced=document.createElementNS(svg.namespaceURI,'polyline');svg.append(namespaced);
 namespaced.setAttributeNS('urn:other','points','20,20 40,40');namespaced.setAttributeNS(null,'points','0,0 3,4');
 out.namespace=[snap(namespaced.points),namespaced.getTotalLength(),namespaced.getBBox().width,namespaced.getBBox().height];
 return out;
 });
 fs.writeFileSync(path.resolve(__dirname,label+'.json'),JSON.stringify(result,null,2))
 }finally{if(browser)await browser.close();if(proc)proc.kill()}
})().catch(e=>{console.error(e);process.exit(1)});
