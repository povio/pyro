// Serves variants of a captured TOS Positions page (TOS_POSITIONS_HTML, see tos-capture-positions.mjs) with N rows
// (the captured rows cycled) and an in-page timing script.
// GET /?rows=1000&mode=style|memo|append|layout&loops=5
import http from 'node:http';
import { readFileSync } from 'node:fs';
const html = readFileSync(process.env.TOS_POSITIONS_HTML ?? '/tmp/pyro-startup/positions.html', 'utf8');
const tbStart = html.indexOf('<tbody'), tbOpenEnd = html.indexOf('>', tbStart) + 1, tbEnd = html.indexOf('</tbody>');
const head = html.slice(0, tbOpenEnd), tail = html.slice(tbEnd);
const rows = html.slice(tbOpenEnd, tbEnd).match(/<tr[\s\S]*?<\/tr>/g);
const script = (mode, loops) => `<script>
const t=()=>performance.now(); const els=Array.from(document.querySelectorAll('tbody *')); const all=Array.from(document.querySelectorAll('body *'));
const out={mode:'${mode}',rowElements:els.length,elements:all.length,loops:${loops}}; let t0; const body=document.body;
if('${mode}'==='style'){ for(const e of all)getComputedStyle(e).color; t0=t(); for(let k=0;k<${loops};k++){body.classList.toggle('pyro-bench'); for(const e of all)getComputedStyle(e).color;} out.full_restyle_ms_per_loop=+((t()-t0)/${loops}).toFixed(2);}
if('${mode}'==='memo'){ for(const e of all)getComputedStyle(e).color; t0=t(); for(let k=0;k<${loops};k++){for(const e of all)getComputedStyle(e).color;} out.memo_read_all_ms_per_loop=+((t()-t0)/${loops}).toFixed(2);}
if('${mode}'==='append'){ const tb=document.querySelector('tbody'); for(const e of all)getComputedStyle(e).color; t0=t(); for(let k=0;k<${loops};k++){const tr=tb.firstElementChild.cloneNode(true); tb.appendChild(tr); getComputedStyle(tr.firstElementChild).color; for(const e of all)getComputedStyle(e).color;} out.append_row_then_read_all_ms_per_loop=+((t()-t0)/${loops}).toFixed(2);}
if('${mode}'==='layout'){ t0=t(); for(const e of els)e.getBoundingClientRect(); out.first_layout_rects_ms=+(t()-t0).toFixed(2); t0=t(); for(const e of els)e.getBoundingClientRect(); out.cached_rects_ms=+(t()-t0).toFixed(2); body.classList.toggle('pyro-bench'); t0=t(); for(const e of els)e.getBoundingClientRect(); out.rects_after_root_toggle_ms=+(t()-t0).toFixed(2);}
console.error('POSBENCH '+JSON.stringify(out));</script>`;
http.createServer((req, res) => {
  const u = new URL(req.url, 'http://x'); const n = +(u.searchParams.get('rows') || 1000);
  let body = ''; for (let i = 0; i < n; i++) body += rows[i % rows.length];
  res.writeHead(200, { 'content-type': 'text/html' });
  res.end(head + body + tail.replace('</body>', script(u.searchParams.get('mode') || 'style', +(u.searchParams.get('loops') || 5)) + '</body>'));
}).listen(9447, '127.0.0.1');
