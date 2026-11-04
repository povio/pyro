// markers: what text does the PDF carry for ::marker
const doc=(b,st='')=>`<!doctype html><body style="margin:0;font:16px/20px Arial;${st}">${b}`;
const C={};
for(const t of ['disc','circle','square','decimal','lower-alpha','upper-roman','none','"-> "','disclosure-open','decimal-leading-zero','georgian','armenian','cjk-decimal','hebrew'])
 C['ul_'+t]=doc(`<ul style="margin:0 0 0 40px;padding:0;list-style-type:${t.includes('"')?t:t}"><li>a1</li><li>a2</li></ul>`);
C.ol=doc(`<ol style="margin:0 0 0 40px;padding:0"><li>a1</li><li>a2</li></ol>`);
C.olstart=doc(`<ol start=7 reversed style="margin:0 0 0 40px;padding:0"><li>a1</li><li>a2</li></ol>`);
C.inside=doc(`<ul style="margin:0;padding:0;list-style-position:inside"><li>a1</li></ul>`);
C.insideol=doc(`<ol style="margin:0;padding:0;list-style-position:inside"><li>a1</li></ol>`);
C.mcontent=`<!doctype html><style>li::marker{content:"X) "}</style><body style="margin:0;font:16px/20px Arial"><ul style="margin:0 0 0 40px;padding:0"><li>a1</li></ul>`;
C.before=`<!doctype html><style>li::before{content:"Y) "}</style><body style="margin:0;font:16px/20px Arial"><ul style="margin:0 0 0 40px;padding:0;list-style:none"><li>a1</li></ul>`;
C.img=doc(`<ul style="margin:0 0 0 40px;padding:0;list-style-image:url(data:image/gif;base64,R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7)"><li>a1</li></ul>`);
C.divli=doc(`<div style="display:list-item;margin-left:40px">a1</div>`);
C.summary=doc(`<details open><summary>s1</summary>a1</details>`);
C.summaryc=doc(`<details><summary>s1</summary>a1</details>`);
export const cases=Object.entries(C).map(([name,html])=>({name,html}));
