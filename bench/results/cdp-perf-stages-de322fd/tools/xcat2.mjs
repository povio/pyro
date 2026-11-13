// usage: bun xcat2.mjs trace.xml cpuUsPerOp [topN]
// Innermost-first: the first stack frame (from the leaf up) that matches a rule decides the bucket.
import fs from 'node:fs';
import { demangle } from './demangle.mjs';
const [file, cpuUs, topN = 20] = process.argv.slice(2);
const x = fs.readFileSync(file, 'utf8');
const RULES = [
  [/sleep<pyro_browser|post<pyro_browser|wake::Sleeper|wake::Notifier|LoopHandle::post|wake_by_val|wake_by_ref|park_internal|Driver::turn|Driver::park|unpark|Inject::push/, 'plumbing: cross-thread wake/park (pipe, kevent)'],
  [/TcpStream|write_all|poll_read_priv|poll_write_priv/, 'plumbing: socket syscalls'],
  [/^pyro_cdp::ws::|ws::Decoder|OutFrame|finish_text|apply_mask|unmask/, 'plumbing: websocket framing'],
  [/Watchdog|CPUTime::forCurrentThread/, 'engine: JSC watchdog timer'],
  [/serde_json|serde::de|serde::ser|ryu|itoa|parse_command|Command::params|to_frame|result_frame|event_frame/, 'plumbing: JSON (serde)'],
  [/Outbox::push|outbox::|INBOX|Inbound::/, 'plumbing: channels/outbox'],
  [/tokio::|mio::|batch_semaphore|scheduler::current_thread|runtime::task|io::driver|time::Driver|Semaphore/, 'plumbing: tokio runtime'],
  [/selectors::|pyro_dom::select|select::|matches_|query_selector|QuerySelector/, 'work: selector matching'],
  [/html5ever|pyro_html|tokenizer|TreeBuilder|tree_builder|markup5ever/, 'work: HTML parse'],
  [/cssparser|pyro_css|stylesheet|Stylesheet/, 'work: CSS parse'],
  [/checkpoint|sync_root|FrameHost::|after_task|drain_finalized/, 'browser: event-loop checkpoint'],
  [/remote::|RemoteObject|runtime_protocol/, 'cdp: RemoteObject describe/register'],
  [/Heap::|GCActivity|Collect|Marking|SlotVisitor|Sweep|MarkedBlock|IsoSubspace/, 'engine: JSC GC'],
  [/Parser<|JSC::parse|BytecodeGenerator|CodeBlock|UnlinkedCodeBlock|SourceCodeKey|CodeCache|StringImpl::hash|Lexer/, 'engine: JSC parse/compile/codecache'],
  [/pyro_net|hyper|h2::|reqwest|http::|pyro_fetch|fetch::/, 'work: network/fetch'],
  [/pyro_webapi|pyro_dom|pyro_browser|pyro_js::class|bindings|Document|Element|Node/, 'work: DOM/webapi + bindings'],
  [/pyro_js::|JSObjectCall|JSEvaluateScript|JSValue|^JSC::|^WTF::|LLInt|llint|^Yarr|bmalloc|pas_/, 'engine: JSC execute'],
  [/pyro_cdp::|target::|dispatch|EngineHost|Browser<|handle_text/, 'cdp: dispatch/target logic'],
];
const frames = new Map(), bts = new Map(); const cat = new Map(), self = new Map(); let total = 0;
for (const r of x.split('<row>').slice(1)) {
  let st; const bt = r.match(/<tagged-backtrace id="(\d+)">([\s\S]*?)<\/tagged-backtrace>/);
  if (bt) { st = []; for (const f of bt[2].matchAll(/<frame (?:id="(\d+)"[^>]*?name="([^"]*)"|ref="(\d+)")/g)) { if (f[1]) { frames.set(f[1], f[2]); st.push(f[2]); } else st.push(frames.get(f[3])); } bts.set(bt[1], st); }
  else st = bts.get(r.match(/<tagged-backtrace ref="(\d+)"/)?.[1]);
  if (!st || !st.length) continue; total++;
  const d = st.map((s) => demangle(s ?? '?').replace(/&lt;/g, '<').replace(/&gt;/g, '>').replace(/&amp;/g, '&'));
  let c = 'other';
  outer: for (const f of d) for (const [re, name] of RULES) if (re.test(f)) { c = name; break outer; }
  if (c === 'other' && /^0x|^\?\?\?|^$/.test(d[0])) c = 'engine: JSC execute (JIT code)';
  cat.set(c, (cat.get(c) ?? 0) + 1);
  self.set(d[0].slice(0, 100), (self.get(d[0].slice(0, 100)) ?? 0) + 1);
}
const ns = (Number(cpuUs) * 1000) / total;
console.log(`samples ${total}, ~${ns.toFixed(0)} ns/op per sample`);
for (const [k, v] of [...cat].sort((a, b) => b[1] - a[1])) if (v * ns >= 10) console.log(`${(100 * v / total).toFixed(1).padStart(5)}% ${(v * ns).toFixed(0).padStart(8)} ns/op  ${k}`);
console.log('  -- top self --');
for (const [k, v] of [...self].sort((a, b) => b[1] - a[1]).slice(0, Number(topN))) console.log(`${(100 * v / total).toFixed(1).padStart(5)}% ${(v * ns).toFixed(0).padStart(8)} ns/op  ${k}`);
