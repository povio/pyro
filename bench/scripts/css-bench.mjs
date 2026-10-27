// Synthetic style/layout benchmark page. Serve: node css-bench.mjs (port 9445); load with
// LIGHTNINGPANDA_GEOMETRY=flow lightningpanda fetch --dump none "http://127.0.0.1:9445/?n=2000&rules=1500&mode=style&loops=5"
// Modes: style (full restyle per loop), memo (cached reads), offset, rects, matches, qsa, relayout, append.
// Options: nonth=1 drops :nth-child rules, noattr=1 drops [data-k] rules. Results print via console.error.
import http from 'node:http';
function css(rules,nonth,noattr){const o=[];for(let i=0;i<rules;i++){const k=i%10;
 if(k<4)o.push(`.c${i}{color:rgb(${i%255},0,0);padding:${i%7}px;margin:${i%5}px}`);
 else if(k<6)o.push(`.row .c${i} > span{font-weight:bold;display:inline-block}`);
 else if(k<7)o.push(`div.c${i}:hover{background:#eee}`);
 else if(k<8)o.push(nonth?`.list > .item .c${i}{opacity:.9}`:`.list > .item:nth-child(${(i%4)+1}n) .c${i}{opacity:.9}`);
 else if(k<9)o.push(`#root .c${i} .leaf{width:${i%300}px;height:20px}`);
 else o.push(noattr?`.c${i}{border:1px solid #000}`:`[data-k="${i}"]{border:1px solid #000}`);} return o.join('\n');}
function body(n,rules){let b='<div id="root"><ul class="list">';for(let i=0;i<n;i++)b+=`<li class="item row c${i%rules} c${(i*7)%rules}" data-k="${i}"><span class="c${(i*3)%rules}">x</span><div class="leaf c${(i*11)%rules}">leaf ${i}</div></li>`;return b+'</ul></div>';}
function script(mode,loops){return `
const t=()=>performance.now(); const els=Array.from(document.querySelectorAll('#root *')); const leaves=Array.from(document.querySelectorAll('.leaf')); const root=document.getElementById('root');
const out={mode:'${mode}',n:els.length,loops:${loops}}; let t0;
if('${mode}'==='style'){ for(const e of els)getComputedStyle(e).color; t0=t(); for(let k=0;k<${loops};k++){root.classList.toggle('c2'); for(const e of els)getComputedStyle(e).color;} out.full_restyle_ms_per_loop=+((t()-t0)/${loops}).toFixed(2);}
if('${mode}'==='memo'){ for(const e of els)getComputedStyle(e).color; t0=t(); for(let k=0;k<${loops};k++){for(const e of els)getComputedStyle(e).color;} out.memo_query_all_ms_per_loop=+((t()-t0)/${loops}).toFixed(2);}
if('${mode}'==='offset'){ for(const e of leaves)e.offsetWidth; t0=t(); for(let k=0;k<${loops};k++){for(const e of leaves)e.offsetWidth;} out.offsetWidth_ms_per_loop=+((t()-t0)/${loops}).toFixed(2); out.leaves=leaves.length;}
if('${mode}'==='rects'){ for(const e of leaves)e.getBoundingClientRect(); t0=t(); for(let k=0;k<${loops};k++){for(const e of leaves)e.getBoundingClientRect();} out.rects_ms_per_loop=+((t()-t0)/${loops}).toFixed(2); out.leaves=leaves.length;}
if('${mode}'==='matches'){ t0=t(); for(let k=0;k<${loops};k++){for(const e of els)e.matches('.leaf');} out.matches_ms_per_loop=+((t()-t0)/${loops}).toFixed(2);}
if('${mode}'==='qsa'){ t0=t(); for(let k=0;k<${loops};k++){document.querySelectorAll('.list > .item:nth-child(2n) .leaf').length;} out.qsa_nth_ms_per_loop=+((t()-t0)/${loops}).toFixed(2); t0=t(); for(let k=0;k<${loops};k++){document.querySelectorAll('.leaf').length;} out.qsa_class_ms_per_loop=+((t()-t0)/${loops}).toFixed(2);}
if('${mode}'==='relayout'){ for(const e of leaves)e.getBoundingClientRect(); t0=t(); for(let k=0;k<${loops};k++){leaves[k%leaves.length].style.width=(100+k)+'px'; leaves[(k*7)%leaves.length].getBoundingClientRect();} out.leaf_width_change_then_rect_ms_per_loop=+((t()-t0)/${loops}).toFixed(2);}
if('${mode}'==='append'){ const ul=document.querySelector('.list'); for(const e of els)getComputedStyle(e).color; t0=t(); for(let k=0;k<${loops};k++){const li=document.createElement('li'); li.className='item row c5'; li.innerHTML='<span class="c9">y</span><div class="leaf c3">new</div>'; ul.appendChild(li); getComputedStyle(li).color; for(const e of els)getComputedStyle(e).color;} out.append_one_then_requery_all_ms_per_loop=+((t()-t0)/${loops}).toFixed(2);}
console.error('CSSBENCH '+JSON.stringify(out));`;}
http.createServer((req,res)=>{const u=new URL(req.url,'http://x');const n=+(u.searchParams.get('n')||2000),rules=+(u.searchParams.get('rules')||1500);res.writeHead(200,{'content-type':'text/html'});res.end(`<!doctype html><html><head><style>${css(rules,u.searchParams.get('nonth'),u.searchParams.get('noattr'))}</style></head><body>${body(n,rules)}<script>${script(u.searchParams.get('mode')||'style',+(u.searchParams.get('loops')||10))}</script></body></html>`);}).listen(9445,'127.0.0.1');
