// <br> in flex and grid containers: a line of an anonymous item whose CSS is ignored, except display:none and contents
const P='<p>A</p>', Q='<p>B</p>';
const CONT={col:'display:flex;flex-direction:column;align-items:flex-start',stretch:'display:flex;flex-direction:column',row:'display:flex',wrap:'display:flex;flex-wrap:wrap;width:24px',grid:'display:grid',gap:'display:flex;flex-direction:column;gap:10px'};
const br=s=>`<br style="${s}">`;
const INNER={
  one:P+'<br>'+Q, alone:'<br>Z', two:P+'<br><br>'+Q, three:P+'<br><br><br>'+Q, comment:P+'<br><!--c--><br>'+Q,
  text_before:P+'tx<br>'+Q, text_after:P+'<br>ty'+Q, text_both:P+'tx<br>ty'+Q, text_two:P+'tx<br><br>ty'+Q, trailing:P+'tx<br><br>'+Q,
  span_around:P+'<span>sp</span><br><span>sq</span>'+Q, span_in:P+'<span>sp<br>sq</span>'+Q, span_only:P+'<span><br></span>'+Q, span_text:P+'tx<span>sp</span><br>'+Q,
  span_font:P+'<span style="font-size:40px;line-height:50px"><br></span>'+Q, div_only:P+'<div><br></div>'+Q, div_text:P+'<div>tx<br></div>'+Q,
  lh40:P+br('line-height:40px')+Q, lh5:P+br('line-height:5px')+Q, fs40:P+br('font-size:40px')+Q, fs5:P+br('font-size:5px')+Q, lh_text:P+'tx'+br('line-height:60px;font-size:50px')+'ty'+Q,
  block:P+br('display:block')+Q, block_box:P+br('display:block;height:50px;width:30px;background:#000')+Q, flex:P+br('display:flex')+Q, grid:P+br('display:grid')+Q, inline:P+br('display:inline')+Q, ib:P+br('display:inline-block')+Q,
  none:P+br('display:none')+Q, contents:P+br('display:contents')+Q, none_text:P+'tx'+br('display:none')+'ty'+Q, contents_text:P+'tx'+br('display:contents')+'ty'+Q,
  box:P+br('margin:10px;padding:7px;border:3px solid #000')+Q, size:P+br('height:50px;width:50px;min-height:30px')+Q, abs:P+br('position:absolute')+Q, float:P+br('float:left')+Q, order:P+br('order:5')+'ty'+Q, grow:P+br('flex:1 0 100px')+Q, self:P+br('align-self:center')+Q, hidden:P+br('visibility:hidden')+Q,
  sib_none:P+'<br><span style="display:none"></span><br>'+Q, sib_none_text:P+'tx<span style="display:none"></span>ty'+Q, sib_abs:P+'<br><span style="position:absolute"></span><br>'+Q, sib_abs_text:P+'tx<span style="position:absolute"></span>ty'+Q,
  wrap_contents:P+'<div style="display:contents"><br></div>'+Q, wrap_contents2:P+'<br><div style="display:contents"><br></div><br>'+Q, wrap_contents_text:P+'tx<div style="display:contents">ty<br>tz</div>tw'+Q, wrap_contents_block:P+'<div style="display:contents"><br><p>C</p><br></div>'+Q,
  pseudo:'<style>#c::before{content:"pp"} #c::after{content:"qq"}</style>tx<br>ty', first_last:'<br><div>A</div><br>',
};
const WS={pre_wrap:'white-space:pre-wrap',pre:'white-space:pre',pre_line:'white-space:pre-line',break_spaces:'white-space:break-spaces',preserve:'white-space-collapse:preserve',normal:''};
const SPACES={space_after:P+'<br> '+Q, nl_after:P+'<br>\n'+Q, space_before:P+' <br>'+Q, nl_before:P+'\n<br>'+Q, around:P+'\n<br>\n'+Q, between_space:P+'<br> <br>'+Q, between_nl:P+'<br>\n<br>\n'+Q, first:' <br>'+Q, span_space:'<span>s</span> <br><span>t</span>', br_space_span:'<br> <span>t</span>', comment_space:'<br><!--c--> <br>'+Q, none_space:'<br><span style="display:none"></span> <br>'+Q, text_nl:'tx\n'+Q, text_space:P+' tx <br> ty '+Q, only:P+' \n '+Q};
const FONTS={lh15:'font:15px/15px Arial',normal:'font:16px/normal Arial',f13:'font:13.3333px/1.5 Arial',f11:'font:11.2px/1.4 Arial',times:'font:16px/normal "Times New Roman"'};
const page=(style,inner)=>`<!doctype html><body style="margin:0;font:16px/20px Arial"><div id=c style="width:300px;${style}">${inner}</div><div>ZZ</div>`;
export const cases=[];
for(const [c,cs] of Object.entries(CONT)) for(const [n,h] of Object.entries(INNER)) cases.push({name:`${c}|${n}`,html:page(cs,h)});
for(const c of ['col','row','grid']) for(const [w,ws] of Object.entries(WS)) for(const [n,h] of Object.entries(SPACES)) cases.push({name:`ws-${c}|${w}|${n}`,html:page(`${CONT[c]};${ws}`,h)});
for(const c of ['col','row','grid']) for(const [f,fs] of Object.entries(FONTS)) for(const n of ['one','two','text_both']) cases.push({name:`font-${c}|${f}|${n}`,html:page(`${CONT[c]};${fs}`,INNER[n])});
// A plain block has the same line, and ignores the same properties
for(const n of ['one','two','text_both','lh_text','block','block_box','flex','ib','none','contents','none_text','contents_text','box','abs','float','sib_none_text']) cases.push({name:`block|${n}`,html:page('',INNER[n])});
