function () {
 document.body.style.cssText='margin:0;width:2400px;height:3000px';
 document.body.innerHTML='<div id="sc" style="position:absolute;left:100px;top:390px;width:200px;height:120px;overflow:scroll"><div style="position:relative;width:900px;height:900px"><img id="im" style="position:absolute;left:23.75px;top:71.75px;width:30px;height:20px;border:3px solid;padding:4px;transform:translate(16px,12px)"></div></div><img id="fixed" style="position:fixed;left:11px;top:19px;width:5px;height:5px">';
 const sc=document.getElementById('sc'),im=document.getElementById('im'),fixed=document.getElementById('fixed'),rows={};
 function fields(i){const r=i.getBoundingClientRect();return {x:i.x??null,y:i.y??null,offsetLeft:i.offsetLeft,offsetTop:i.offsetTop,rectX:r.x,rectY:r.y}}
 function snap(k){rows[k]={image:fields(im),fixed:fields(fixed),scroll:[sc.scrollLeft,sc.scrollTop,window.scrollX,window.scrollY]}}
 snap('initial');sc.scrollLeft=13;sc.scrollTop=31;snap('nested');window.scrollTo(30,200);snap('window');im.style.left='-17.75px';im.style.top='98.25px';snap('relayout');im.style.zoom='2';snap('zoom');im.style.display='none';snap('hidden');im.style.display='block';im.remove();snap('detached');return rows
}
