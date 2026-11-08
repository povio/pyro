//! Review stress tests for the raw JS values dispatch and bindings hold
//! (perf/events-and-style): the realm's current event cell, the listener
//! functions mirrored by slot beside the wrapper, the id/className strings
//! kept per wrapper, and payloads drained inside long tasks. Each test forces
//! synchronous collections (`gc()`, JSC's debugging SPI) at the points where
//! a value is held only by those structures, then reads it back.

use lp_browser::*;
use lp_js::Realm;
use std::time::{Duration, Instant};

fn eval(realm: &Realm, source: &str) -> String {
    let scope = realm.scope().unwrap();
    scope
        .eval(source, Some("events-gc-review.js"), 1)
        .unwrap_or_else(|e| panic!("{}: {}", source, e.details(&scope).text()))
        .to_string()
        .unwrap()
}

/// `gc()`: a synchronous full collection, callable from any point in script.
fn install_gc(realm: &Realm) {
    let s = realm.scope().unwrap();
    let f = s
        .function("gc", |s, _this, _args| {
            if let Some(gc) = lp_js::sys::spi::get().synchronous_gc_for_debugging {
                // SAFETY: the scope's live context, on the engine thread.
                unsafe { gc(s.ctx()) };
            }
            Ok(s.undefined())
        })
        .unwrap();
    s.global().set("gc", f.value()).unwrap();
}

fn page() -> (BrowserThread, PageId, Realm) {
    let mut t = crate::browser_thread().unwrap();
    let page = t.browser.create_page().unwrap();
    let root = t.browser.page(page).unwrap().main_frame.main_realm().unwrap().clone();
    install_gc(&root);
    t.browser.checkpoint();
    (t, page, root)
}

fn settle(t: &mut BrowserThread) {
    t.run_until_idle(Some(Instant::now() + Duration::from_secs(3)));
}

/// Runs the page until `done` holds, at most 120 s (debug builds are slow).
fn run_until(t: &mut BrowserThread, root: &Realm, done: &str) {
    let deadline = Instant::now() + Duration::from_secs(120);
    while Instant::now() < deadline {
        t.run_until_idle(Some(Instant::now() + Duration::from_millis(50)));
        if eval(root, done) == "true" {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("timed out waiting for {done}");
}

#[test]
fn window_event_survives_collections_in_nested_listeners() {
    let (mut t, _page, root) = page();
    assert_eq!(
        eval(
            &root,
            "(() => { let ok = 0, bad = []; const a = document.createElement('div'), b = document.createElement('span');
               a.append(b); document.body.append(a);
               for (let i = 0; i < 150; i++) {
                 const outer = () => { gc(); const ev = window.event; a.dispatchEvent(new CustomEvent('i', { detail: { n: i } }));
                   gc(); const junk = []; for (let k = 0; k < 2000; k++) junk.push({ k });
                   if (window.event === ev && ev.type === 'click' && ev.target === b) ok++; else bad.push('o' + i); };
                 const inner = () => { gc(); const e = window.event; if (e && e.type === 'i' && e.detail.n === i) ok++; else bad.push('i' + i);
                   b.dispatchEvent(new Event('deep')); gc(); if (window.event === e) ok++; else bad.push('r' + i); };
                 const deep = () => { gc(); if (window.event.type === 'deep') ok++; };
                 b.addEventListener('click', outer); a.addEventListener('i', inner); b.addEventListener('deep', deep);
                 b.click();
                 b.removeEventListener('click', outer); a.removeEventListener('i', inner); b.removeEventListener('deep', deep);
                 gc();
                 if (window.event !== undefined) bad.push('after' + i);
               }
               return ok + ':' + bad.slice(0, 5).join(); })()"
        ),
        "600:"
    );
    settle(&mut t);
}

#[test]
fn window_event_survives_collections_in_error_listeners_of_a_throwing_listener() {
    let (mut t, _page, root) = page();
    assert_eq!(
        eval(
            &root,
            "(() => { const seen = []; let n = 0;
               window.addEventListener('error', e => { gc(); const junk = []; for (let k = 0; k < 5000; k++) junk.push([k]); gc();
                 seen.push(window.event === e ? 'E' : 'x'); e.preventDefault(); });
               const b = document.createElement('button'); document.body.append(b);
               b.addEventListener('click', () => { gc(); throw new Error('boom' + n); });
               b.addEventListener('click', () => { gc(); const ev = window.event; seen.push(ev && ev.type === 'click' && ev.isTrusted === false ? 'C' : 'y'); });
               for (n = 0; n < 100; n++) { b.click(); gc(); }
               seen.push(window.event === undefined);
               return seen.filter(x => x === 'E').length + ':' + seen.filter(x => x === 'C').length + ':' + seen[seen.length - 1]; })()"
        ),
        "100:100:true"
    );
    settle(&mut t);
}

#[test]
fn window_event_survives_collections_when_a_trusted_event_is_only_on_the_native_stack() {
    let (mut t, _page, root) = page();
    // A message event fired from a task (depth 0): its wrapper is referenced
    // only by the dispatch frame and the realm's raw cell.
    eval(
        &root,
        "globalThis.msgs = []; globalThis.done = 0;
         window.addEventListener('message', () => { gc(); const junk = []; for (let k = 0; k < 20000; k++) junk.push(document.createElement('i'));
           junk.length = 0; gc(); const e = window.event; msgs.push(e && e.type === 'message' && e.data === done ? 1 : 0);
           if (e) { try { throw new Error('x' + e.data); } catch (_) {} }
           done++; });
         window.addEventListener('message', () => { gc(); msgs.push(window.event.data === done - 1 ? 1 : 0); });
         for (let i = 0; i < 40; i++) postMessage(i, '*'); 0",
    );
    run_until(&mut t, &root, "done === 40");
    assert_eq!(
        eval(&root, "msgs.length + ':' + msgs.every(x => x === 1) + ':' + (window.event === undefined)"),
        "80:true:true"
    );
}

#[test]
fn listener_slots_reused_after_removal_inside_listeners_call_the_new_functions() {
    let (mut t, _page, root) = page();
    assert_eq!(
        eval(
            &root,
            "(() => { const a = document.createElement('div'); document.body.append(a); let log = [];
               for (let i = 0; i < 300; i++) {
                 // Functions referenced only by the target's listener list.
                 a.addEventListener('t', function self() { a.removeEventListener('t', self); gc(); log.push('s' + i); });
                 a.addEventListener('t', (() => { const big = new Array(100).fill(i); return () => { gc(); log.push('k' + big[99]); }; })(), { once: true });
                 const obj = { n: i, handleEvent() { gc(); log.push('h' + this.n); a.removeEventListener('t', obj); } };
                 a.addEventListener('t', obj);
                 gc();
                 a.dispatchEvent(new Event('t'));
                 a.dispatchEvent(new Event('t'));
               }
               const want = []; for (let i = 0; i < 300; i++) want.push('s' + i, 'k' + i, 'h' + i);
               return log.join() === want.join() ? 'ok' : log.slice(0, 12).join(); })()"
        ),
        "ok"
    );
    settle(&mut t);
}

#[test]
fn listeners_added_inside_listeners_and_handlers_survive_collections() {
    let (mut t, _page, root) = page();
    assert_eq!(
        eval(
            &root,
            "(() => { const a = document.createElement('div'); document.body.append(a); const log = [];
               let depth = 0;
               const adder = () => { if (depth++ < 50) { const d = depth; a.addEventListener('t', () => { gc(); log.push(d); }); } gc(); };
               a.addEventListener('t', adder);
               for (let i = 0; i < 50; i++) { a.dispatchEvent(new Event('t')); gc(); }
               // handler attribute set, replaced and cleared with collections between
               let h = 0; for (let i = 0; i < 100; i++) { a.onclick = (i % 3) ? (() => { gc(); h++; }) : null; gc(); a.click(); }
               a.setAttribute('onfoo', 'h += 1000');
               return [log.length, h].join(); })()"
        ),
        // listener k (added during dispatch k) runs in dispatches k+1..50: sum 49..0
        "1225,66"
    );
    settle(&mut t);
}

/// Pre-existing on 34f0f2ba (fails there too, worse): a listener added while
/// its node is detached does not pin the wrapper; connecting the node pins it
/// only at the next `after_task`, so a collection earlier in the same task
/// drops the wrapper and its listener array, and the listeners never fire.
#[test]
#[ignore]
fn listener_functions_held_only_by_targets_survive_collections() {
    let (mut t, _page, root) = page();
    eval(
        &root,
        "globalThis.count = 0;
         (() => { const box = document.createElement('section'); box.id = 'box'; document.body.append(box);
           for (let i = 0; i < 2000; i++) { const d = document.createElement('div'); d.className = 'c' + i;
             const captured = { i, pad: new Array(20).fill(i) };
             d.addEventListener('ping', () => { count += captured.pad[19] === i ? 1 : 1e6; });
             d.addEventListener('ping', { handleEvent: () => { count += captured.i === i ? 1 : 1e6; } });
             box.append(d); } })(); 0",
    );
    for _ in 0..3 {
        eval(&root, "gc(); (() => { const j = []; for (let k = 0; k < 100000; k++) j.push({ k }); })(); gc(); 0");
        t.browser.checkpoint();
        t.browser.engine.collect_garbage_now();
        t.browser.engine.drain_finalized();
    }
    assert_eq!(
        eval(
            &root,
            "for (const d of document.getElementById('box').children) d.dispatchEvent(new Event('ping')); count"
        ),
        "4000"
    );
    settle(&mut t);
}

#[test]
fn a_target_dropped_by_its_own_listener_keeps_dispatching() {
    let (mut t, _page, root) = page();
    assert_eq!(
        eval(
            &root,
            "(() => { const log = [];
               for (let i = 0; i < 200; i++) {
                 let outer = document.createElement('div'), x = document.createElement('p'); outer.append(x); document.body.append(outer);
                 x.addEventListener('t', function () { outer.remove(); x.remove(); outer = null; x = null; gc(); log.push(this.nodeName); });
                 x.addEventListener('t', function (e) { gc(); log.push(this.nodeName + e.currentTarget.nodeName); });
                 outer.addEventListener('t', function (e) { gc(); log.push(e.currentTarget.nodeName + e.target.nodeName); });
                 x.dispatchEvent(new Event('t', { bubbles: true }));
               }
               gc();
               return [log.length, log.slice(0, 3).join()].join(); })()"
        ),
        "600,P,PP,DIVP"
    );
    settle(&mut t);
}

#[test]
fn detached_targets_with_listeners_are_collected_safely() {
    let (mut t, _page, root) = page();
    eval(
        &root,
        "globalThis.fired = 0; globalThis.kept = [];
         for (let i = 0; i < 20000; i++) { const d = document.createElement('div'); d.addEventListener('x', () => fired++);
           if (i % 1000 === 0) kept.push(d); } 0",
    );
    for _ in 0..3 {
        eval(&root, "gc(); 0");
        t.browser.checkpoint();
        t.browser.engine.collect_garbage_now();
        t.browser.engine.drain_finalized();
    }
    assert_eq!(eval(&root, "kept.forEach(d => d.dispatchEvent(new Event('x'))); fired"), "20");
    settle(&mut t);
}

#[test]
fn an_iframe_removed_by_its_own_listener_mid_dispatch() {
    let (mut t, _page, root) = page();
    for round in 0..15 {
        eval(
            &root,
            "globalThis.f = document.createElement('iframe'); document.body.appendChild(f); globalThis.W = f.contentWindow; 0",
        );
        settle(&mut t);
        let r = eval(
            &root,
            "(() => { const out = []; const d = W.document; const el = d.createElement('div'); d.body.append(el);
               W.eval(`globalThis.inner = () => { top.gc(); top.f.remove(); top.gc(); top.junk = []; for (let k = 0; k < 3000; k++) top.junk.push({k}); top.gc(); };
                        globalThis.outer = () => { top.out.push(window.event && window.event.type); document.body.firstChild.dispatchEvent(new Event('in')); top.gc();
                          top.out.push(String(window.event && window.event.type)); };`);
               globalThis.out = out;
               el.addEventListener('in', W.inner);
               el.addEventListener('in', () => out.push('second-in'));
               el.addEventListener('o', W.outer);
               el.addEventListener('o', () => out.push('second-o'));
               el.dispatchEvent(new Event('o'));
               gc();
               out.push(String(W.event), String(window.event));
               gc();
               out.push(String(W.event));
               return out.join(); })()",
        );
        // Recorded for comparison with base: what runs after the frame is gone.
        if round == 0 {
            eprintln!("iframe-removed-mid-dispatch: {r}");
        }
        assert!(r.starts_with("o,"), "{r}");
        t.browser.checkpoint();
        t.browser.engine.collect_garbage_now();
        t.browser.checkpoint();
    }
    eval(&root, "gc(); 0");
    settle(&mut t);
}

#[test]
fn cross_realm_listener_sets_its_own_windows_event_across_collections() {
    let (mut t, _page, root) = page();
    eval(&root, "globalThis.f = document.createElement('iframe'); document.body.appendChild(f); globalThis.A = f.contentWindow; 0");
    settle(&mut t);
    assert_eq!(
        eval(
            &root,
            "(() => { let ok = 0;
               A.eval('globalThis.l = function (e) { top.gc(); if (window.event === e && top.event === undefined) top.okInc(); top.gc(); }');
               globalThis.okInc = () => ok++;
               const b = document.createElement('b'); document.body.append(b); b.addEventListener('z', A.l);
               for (let i = 0; i < 200; i++) { b.dispatchEvent(new Event('z')); if (i % 10 === 0) gc(); }
               return ok + ':' + (A.event === undefined); })()"
        ),
        "200:true"
    );
    settle(&mut t);
}

#[test]
fn id_and_class_strings_follow_every_attribute_path() {
    let (mut t, _page, root) = page();
    eval(&root, "globalThis.f = document.createElement('iframe'); document.body.appendChild(f); 0");
    settle(&mut t);
    assert_eq!(
        eval(
            &root,
            "(() => { const out = []; const a = document.createElement('div'); document.body.append(a);
               const rd = () => { const x = a.id; gc(); const y = a.id; return x === y ? y : 'MISMATCH'; };
               a.id = 'one'; out.push(rd(), rd());
               a.setAttribute('id', 'two'); out.push(rd());
               a.getAttributeNode('id').value = 'three'; out.push(rd());
               a.getAttributeNode('id').nodeValue = 'four'; out.push(rd());
               a.getAttributeNode('id').textContent = 'five'; out.push(rd());
               a.attributes.id.value = 'one'; out.push(rd());
               const at = document.createAttribute('id'); at.value = 'six'; a.setAttributeNode(at); out.push(rd());
               at.value = 'seven'; out.push(rd());
               a.removeAttributeNode(at); out.push(rd() === '');
               a.setAttributeNS(null, 'id', 'eight'); out.push(rd());
               a.setAttributeNS('urn:x', 'x:id', 'nine'); out.push(rd());
               a.removeAttribute('id'); out.push(rd() === '');
               a.toggleAttribute('id'); out.push(rd() === '');
               a.id = 'back'; out.push(rd());
               const p = document.createElement('div'); p.innerHTML = '<span id=s1 class=\"a b\"></span>'; const s1 = p.firstChild; out.push(s1.id, s1.id, s1.className);
               p.innerHTML = '<span id=s1 class=a></span>'; out.push(p.firstChild.id, p.firstChild.id, s1 !== p.firstChild, s1.id);
               // adoption across documents, read from both realms
               const D = f.contentDocument; D.adoptNode(a); D.body.append(a); out.push(a.id, a.id, f.contentWindow.eval('document.body.lastChild.id'), f.contentWindow.eval('document.body.lastChild.id'));
               a.id = 'adopted'; out.push(a.id, f.contentWindow.eval('document.body.lastChild.id'));
               // className tokens
               const c = document.createElement('div');
               c.className = 'x'; out.push(c.className, c.className); c.classList.add('y'); out.push(c.className, c.className);
               c.classList.remove('x'); out.push(c.className, c.className); c.className = '  y  '; out.push('[' + c.className + ']', '[' + c.className + ']');
               c.classList.toggle('y'); out.push('[' + c.className + ']'); c.className = 'y'; out.push(c.className, c.className);
               // getter applied cross-realm
               const g = Object.getOwnPropertyDescriptor(f.contentWindow.Element.prototype, 'id').get; c.id = 'cr'; out.push(g.call(c), g.call(c));
               // svg id (className is a string on SVG elements on base and branch alike; Chrome: SVGAnimatedString)
               const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg'); svg.setAttribute('id', 'q'); out.push(svg.id, svg.id);
               return out.join('|'); })()"
        ),
        "one|one|two|three|four|five|one|six|seven|true|eight|eight|true|true|back|s1|s1|a b|s1|s1|true|s1|back|back|back|back|adopted|adopted|x|x|x y|x y|y|y|[  y  ]|[  y  ]|[]|y|y|cr|cr|q|q"
    );
    settle(&mut t);
}

#[test]
fn many_wrappers_with_read_ids_are_released() {
    let (mut t, _page, root) = page();
    let before = eval(&root, "0");
    let _ = before;
    eval(
        &root,
        "for (let r = 0; r < 5; r++) { const box = document.createElement('div');
           for (let i = 0; i < 20000; i++) { const d = document.createElement('div'); d.id = 'id' + i; d.className = 'c' + (i % 7); box.append(d); }
           let n = 0; for (const d of box.children) { n += d.id.length + d.id.length + d.className.length + d.className.length; }
           box.remove(); } 0",
    );
    for _ in 0..4 {
        t.browser.checkpoint();
        t.browser.engine.collect_garbage_now();
        t.browser.engine.drain_finalized();
        t.browser.checkpoint();
    }
    let live = eval(&root, "document.querySelectorAll('*').length");
    assert!(live.parse::<u32>().unwrap() < 20, "{live}");
    settle(&mut t);
}

#[test]
fn long_task_drain_keeps_held_nodes_and_mutation_records() {
    let (mut t, _page, root) = page();
    eval(
        &root,
        "globalThis.recs = []; globalThis.mo = new MutationObserver(rs => { for (const r of rs) recs.push(r); });
         mo.observe(document.body, { childList: true, subtree: true, attributes: true, characterData: true });
         globalThis.held = []; globalThis.weak = []; globalThis.finalized = 0;
         globalThis.fr = new FinalizationRegistry(() => finalized++);
         (() => {
           for (let i = 0; i < 300000; i++) {
             const d = document.createElement('div');
             if (i % 1000 === 0) { d.expando = i; d.setAttribute('data-i', i); d.append(document.createTextNode('t' + i)); held.push(d); }
             if (i % 1000 === 1) { d.setAttribute('data-i', i); document.body.append(d); d.textContent = 'r' + i; d.remove(); }
             if (i % 1000 === 2) { weak.push(new WeakRef(d)); fr.register(d, i); }
           }
         })();
         globalThis.syncCheck = held.every((d, k) => d.expando === k * 1000 && d.getAttribute('data-i') == k * 1000 && d.textContent === 't' + k * 1000 && d.parentNode === null)
           && weak.every(w => w.deref() !== undefined);
         0",
    );
    assert_eq!(eval(&root, "syncCheck"), "true");
    settle(&mut t);
    for _ in 0..3 {
        t.browser.engine.collect_garbage_now();
        t.browser.checkpoint();
        settle(&mut t);
    }
    assert_eq!(
        eval(
            &root,
            "(() => { const removed = recs.filter(r => r.type === 'childList' && r.removedNodes.length);
               const okRemoved = removed.every(r => { const n = r.removedNodes[0]; return n.nodeName === 'DIV' && n.textContent === 'r' + n.getAttribute('data-i') && n.parentNode === null; });
               const ids = new Set(removed.map(r => r.removedNodes[0].getAttribute('data-i')));
               const okHeld = held.every((d, k) => d.expando === k * 1000 && d.getAttribute('data-i') == k * 1000 && d.textContent === 't' + k * 1000);
               return [removed.length, ids.size, okRemoved, okHeld].join(); })()"
        ),
        "300,300,true,true"
    );
}

#[test]
fn long_task_drain_keeps_nodes_reachable_only_through_dom_objects() {
    let (mut t, _page, root) = page();
    assert_eq!(
        eval(
            &root,
            "(() => {
               const ev = new Event('x'); const r = new Range();
               let coll, walker, related, abort = new AbortController(), fired = 0;
               (() => { const d = document.createElement('div'); d.setAttribute('mark', 'evt'); d.dispatchEvent(ev);
                        const p = document.createElement('p'); p.setAttribute('mark', 'range'); p.textContent = 'hello'; r.selectNodeContents(p);
                        const c = document.createElement('div'); c.innerHTML = '<i mark=c1></i><i mark=c2></i>'; coll = c.children;
                        const w = document.createElement('ul'); w.innerHTML = '<li mark=w1></li>'; walker = document.createTreeWalker(w);
                        const s = document.createElement('a'); s.setAttribute('mark', 'signal'); s.addEventListener('y', () => fired++, { signal: abort.signal }); related = new FocusEvent('focus', { relatedTarget: s });
               })();
               // churn: millions of short-lived wrappers, drained mid-task
               for (let i = 0; i < 800000; i++) document.createElement('span');
               const out = [ev.target && ev.target.getAttribute('mark'), r.startContainer.getAttribute('mark'), r.toString(),
                 coll.length, coll[1] && coll[1].getAttribute('mark'), walker.root.nodeName, walker.firstChild() && walker.currentNode.getAttribute('mark'),
                 related.relatedTarget.getAttribute('mark')];
               related.relatedTarget.dispatchEvent(new Event('y')); abort.abort(); related.relatedTarget.dispatchEvent(new Event('y'));
               out.push(fired);
               return out.join(); })()"
        ),
        "evt,range,hello,2,c2,UL,w1,signal,1"
    );
    settle(&mut t);
}

#[test]
fn long_task_drain_during_a_task_dispatched_listener() {
    let (mut t, _page, root) = page();
    // Listeners of a task-fired event (depth 0 dispatch): the drain runs
    // between their native calls, with the dispatch's Rust frame suspended.
    eval(
        &root,
        "globalThis.res = []; globalThis.n = 0;
         const mk = (i) => { const d = document.createElement('div'); d.id = 'm' + i; document.body.append(d); return d; };
         for (let i = 0; i < 5; i++) mk(i);
         window.addEventListener('message', (e) => {
           const tgt = document.getElementById('m' + e.data);
           tgt.addEventListener('poke', function (pe) { for (let k = 0; k < 100000; k++) document.createElement('b'); res.push(this.id + ':' + pe.target.id + ':' + (window.event === pe)); });
           tgt.addEventListener('poke', function (pe) { res.push('2' + this.id + ':' + (window.event === pe)); });
           for (let k = 0; k < 100000; k++) document.createElement('b');
           tgt.dispatchEvent(new Event('poke'));
           res.push('msg' + e.data + ':' + (window.event === e) + ':' + e.data);
           n++;
         });
         for (let i = 0; i < 5; i++) postMessage(i, '*'); 0",
    );
    run_until(&mut t, &root, "n === 5");
    assert_eq!(
        eval(&root, "res.join()"),
        "m0:m0:true,2m0:true,msg0:true:0,m1:m1:true,2m1:true,msg1:true:1,m2:m2:true,2m2:true,msg2:true:2,m3:m3:true,2m3:true,msg3:true:3,m4:m4:true,2m4:true,msg4:true:4"
    );
}

#[test]
fn custom_element_reactions_survive_a_long_task_drain() {
    let (mut t, _page, root) = page();
    eval(
        &root,
        "globalThis.conn = 0; globalThis.disc = 0;
         customElements.define('x-el', class extends HTMLElement {
           connectedCallback() { conn++; for (let k = 0; k < 3000; k++) document.createElement('b'); }
           disconnectedCallback() { disc++; for (let k = 0; k < 3000; k++) document.createElement('b'); } });
         setTimeout(() => { const box = document.createElement('div'); document.body.append(box);
           box.innerHTML = '<x-el></x-el>'.repeat(300); for (let k = 0; k < 200000; k++) document.createElement('b');
           box.innerHTML = ''; for (let k = 0; k < 200000; k++) document.createElement('b'); globalThis.ceDone = true; }, 0); 0",
    );
    run_until(&mut t, &root, "globalThis.ceDone === true");
    assert_eq!(eval(&root, "conn + ':' + disc"), "300:300");
}

/// Chrome's shape of a computed style, as measured on Chrome 14x (review of
/// C5). Ignored: documents the divergences the branch introduces or keeps;
/// run with `--ignored` to see them.
#[test]
#[ignore]
fn computed_style_shape_matches_chrome() {
    let (_t, _page, root) = page();
    assert_eq!(
        eval(
            &root,
            "(() => { const d = document.createElement('div'); document.body.append(d); const cs = getComputedStyle(d);
               return [cs.hasOwnProperty('color'), String(cs.parentRule), typeof cs.removeProperty, 'mozTransform' in cs, '-moz-transform' in cs,
                       typeof cs.scrollPaddingInline, typeof cs.transformBox, typeof cs.webkitBorderStart, 'fooBar' in cs].join(); })()"
        ),
        "true,null,function,false,false,string,string,string,false"
    );
}

/// Pre-existing on 34f0f2ba (fails there too, worse): a listener added while
/// its node is detached does not pin the wrapper; connecting the node pins it
/// only at the next `after_task`, so a collection earlier in the same task
/// drops the wrapper and its listener array, and the listeners never fire.
#[test]
#[ignore]
fn diag_listener_functions_held_only_by_targets() {
    let (mut t, _page, root) = page();
    eval(
        &root,
        "globalThis.count = 0;
         (() => { const box = document.createElement('section'); box.id = 'box'; document.body.append(box);
           for (let i = 0; i < 200; i++) { const d = document.createElement('div');
             d.addEventListener('ping', () => { count++; }); box.append(d); } })(); 0",
    );
    let fire =
        "count = 0; for (const d of document.getElementById('box').children) d.dispatchEvent(new Event('ping')); count";
    let mut out = vec![eval(&root, fire)];
    eval(&root, "gc(); 0");
    out.push(eval(&root, fire));
    t.browser.checkpoint();
    out.push(eval(&root, fire));
    eval(&root, "(() => { const j = []; for (let k = 0; k < 100000; k++) j.push({ k }); })(); gc(); 0");
    out.push(eval(&root, fire));
    t.browser.engine.collect_garbage_now();
    out.push(eval(&root, fire));
    t.browser.engine.drain_finalized();
    out.push(eval(&root, fire));
    t.browser.checkpoint();
    out.push(eval(&root, fire));
    settle(&mut t);
    out.push(eval(&root, fire));
    assert_eq!(out.join(","), "200,200,200,200,200,200,200,200");
}
