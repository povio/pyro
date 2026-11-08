//! Web Animations: `Element.animate`, `Animation`, `KeyframeEffect`,
//! `document.timeline` and `getAnimations`.

use lp_browser::*;
use lp_js::Realm;
use std::time::{Duration, Instant};

fn eval(realm: &Realm, source: &str) -> String {
    let scope = realm.scope().unwrap();
    scope
        .eval(source, Some("web-animations-test.js"), 1)
        .unwrap_or_else(|e| panic!("{}: {}", source, e.details(&scope).text()))
        .to_string()
        .unwrap()
}

fn page() -> (BrowserThread, Realm) {
    let mut t = crate::browser_thread().unwrap();
    let page = t.browser.create_page().unwrap();
    let root = t.browser.page(page).unwrap().main_frame.main_realm().unwrap().clone();
    (t, root)
}

/// Runs the page until `done` holds, at most 5 s.
fn run_until(thread: &mut BrowserThread, root: &Realm, done: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        thread.run_until_idle(Some(Instant::now() + Duration::from_millis(50)));
        if eval(root, done) == "true" {
            return;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    panic!("timed out waiting for {done}: {}", eval(root, "String(globalThis.log)"));
}

#[test]
fn the_interfaces_are_made_on_first_use() {
    let (_t, root) = page();
    // getAnimations() answers without making the module.
    assert_eq!(eval(&root, "JSON.stringify([document.body.getAnimations(), document.getAnimations()])"), "[[],[]]");
    assert_eq!(
        eval(
            &root,
            "['Animation', 'AnimationEffect', 'KeyframeEffect', 'AnimationTimeline', 'DocumentTimeline',
              'AnimationPlaybackEvent', 'CSSAnimation', 'CSSTransition'].map(n => typeof window[n]).join()"
        ),
        "function,function,function,function,function,function,function,function"
    );
    assert_eq!(
        eval(
            &root,
            "[document.timeline === document.timeline, document.timeline instanceof DocumentTimeline,
              document.timeline instanceof AnimationTimeline, typeof document.timeline.currentTime,
              Math.abs(document.timeline.currentTime - performance.now()) < 50,
              new DocumentTimeline({ originTime: 10 }).currentTime < document.timeline.currentTime,
              Object.prototype.toString.call(document.timeline)].join()"
        ),
        "true,true,true,number,true,true,[object DocumentTimeline]"
    );
    // Abstract interfaces cannot be constructed.
    assert_eq!(
        eval(
            &root,
            "['AnimationEffect', 'AnimationTimeline', 'CSSAnimation', 'CSSTransition'].map(n => {
               try { new window[n](); return 'made'; } catch (e) { return e.name; } }).join()"
        ),
        "TypeError,TypeError,TypeError,TypeError"
    );
    // An assigned global replaces the interface.
    assert_eq!(eval(&root, "window.Animation = 1; String(Animation)"), "1");
}

#[test]
fn an_animation_plays_and_finishes_with_its_promise_and_events() {
    let (mut t, root) = page();
    eval(
        &root,
        "globalThis.log = [];
         globalThis.el = document.body.appendChild(document.createElement('div'));
         globalThis.anim = el.animate([{ opacity: 1 }, { opacity: 0 }], { duration: 30, id: 'fade' });
         log.push('created:' + [anim.playState, anim.pending, anim.currentTime, anim.startTime, anim.id,
           anim.effect.target === el, anim.timeline === document.timeline, anim instanceof EventTarget].join('/'));
         anim.ready.then(a => log.push('ready:' + [a === anim, anim.pending, typeof anim.startTime].join('/')));
         anim.finished.then(a => log.push('finished:' + [a === anim, anim.playState].join('/')));
         anim.onfinish = function (e) { log.push('onfinish:' + [e.type, e.currentTime, e instanceof AnimationPlaybackEvent,
           typeof e.timelineTime, this === anim].join('/')); };
         anim.addEventListener('finish', e => log.push('listener:' + e.type));
         log.push('animations:' + el.getAnimations().length + '/' + document.getAnimations().length); 0",
    );
    run_until(&mut t, &root, "log.some(e => e.startsWith('listener'))");
    assert_eq!(
        eval(&root, "log.join(' ')"),
        "created:running/true/0//fade/true/true/true animations:1/1 ready:true/false/number \
         finished:true/finished onfinish:finish/30/true/number/true listener:finish"
    );
    // Not filling: a finished animation is no longer listed.
    assert_eq!(eval(&root, "[anim.playState, anim.currentTime, el.getAnimations().length].join()"), "finished,30,0");
}

#[test]
fn finish_cancel_pause_and_reverse() {
    let (mut t, root) = page();
    eval(
        &root,
        "globalThis.log = [];
         globalThis.el = document.body.appendChild(document.createElement('div'));
         globalThis.a = el.animate({ opacity: [0, 1] }, { duration: 100000, fill: 'forwards' });
         a.finish();
         log.push('finish:' + [a.playState, a.currentTime, a.pending].join('/'));
         a.onfinish = e => log.push('onfinish:' + e.currentTime);
         // Filling forwards: still listed.
         log.push('listed:' + el.getAnimations().length);
         globalThis.b = el.animate({ opacity: [0, 1] }, 100000);
         b.finished.catch(e => log.push('b.finished:' + e.name));
         b.oncancel = e => log.push('oncancel:' + [e.currentTime, typeof e.timelineTime].join('/'));
         b.cancel();
         log.push('cancel:' + [b.playState, b.currentTime, b.startTime].join('/'));
         globalThis.c = el.animate(null, 1000);
         c.pause();
         log.push('pause:' + [c.playState, c.pending].join('/'));
         c.ready.then(() => { log.push('paused:' + [c.playState, c.currentTime, c.startTime].join('/'));
           c.currentTime = 500; c.reverse();
           log.push('reverse:' + [c.playbackRate, c.playState, c.currentTime].join('/'));
           c.ready.then(() => log.push('reversed:' + c.playbackRate)); }); 0",
    );
    run_until(&mut t, &root, "log.some(e => e.startsWith('oncancel')) && log.some(e => e.startsWith('reversed'))");
    assert_eq!(
        eval(&root, "log.join(' ')"),
        "finish:finished/100000/false listed:1 cancel:idle// pause:paused/true b.finished:AbortError \
         onfinish:100000 oncancel:/number paused:paused/0/ reverse:1/running/500 reversed:-1"
    );
    // Errors of the spec's steps.
    assert_eq!(
        eval(
            &root,
            "const d = el.animate(null, { duration: 10, iterations: Infinity });
             const out = [];
             try { d.finish(); } catch (e) { out.push(e.name); }
             d.playbackRate = 0;
             try { d.finish(); } catch (e) { out.push(e.name); }
             try { d.currentTime = null; } catch (e) { out.push(e.name); }
             out.join()"
        ),
        "InvalidStateError,InvalidStateError,TypeError"
    );
}

#[test]
fn effect_timing_and_keyframes() {
    let (_t, root) = page();
    assert_eq!(
        eval(
            &root,
            "const el = document.body.appendChild(document.createElement('div'));
             const a = el.animate({ opacity: [0, 1], marginLeft: ['0px', '10px', '20px'] },
               { duration: 1000, delay: 100, iterations: 2, direction: 'alternate', easing: 'ease-in', fill: 'both' });
             a.pause();
             a.currentTime = 1350;
             const t = a.effect.getComputedTiming();
             JSON.stringify([t.endTime, t.activeDuration, t.localTime, t.currentIteration, t.fill,
               Math.round(t.progress * 1000) / 1000, a.effect.getTiming().easing,
               a.effect.getKeyframes().map(k => [k.offset, k.computedOffset, k.opacity, k.marginLeft])])"
        ),
        r#"[2100,2000,1350,1,"both",0.622,"ease-in",[[null,0,"0","0px"],[null,0.5,null,"10px"],[null,1,"1","20px"]]]"#
    );
    // Invalid timing and keyframes throw TypeErrors; unknown members are dropped.
    assert_eq!(
        eval(
            &root,
            "const body = document.body;
             const out = [];
             for (const opts of [-1, { duration: 'x' }, { easing: 'bogus' }, { iterations: -1 }, { fill: 'up' }, { delay: NaN }])
               try { body.animate(null, opts); out.push('ok'); } catch (e) { out.push(e.name); }
             try { body.animate([{ offset: 0.6 }, { offset: 0.2 }], 1); } catch (e) { out.push(e.name); }
             const k = new KeyframeEffect(body, [{ notAProperty: 1, cssFloat: 'left', easing: 'steps(2, start)' }], 1);
             out.push(JSON.stringify(k.getKeyframes()));
             out.join(' ')"
        ),
        r#"TypeError TypeError TypeError TypeError TypeError TypeError TypeError [{"offset":null,"computedOffset":1,"easing":"steps(2, start)","composite":"auto","cssFloat":"left"}]"#
    );
}

#[test]
fn commit_styles_writes_the_animated_values() {
    let (_t, root) = page();
    assert_eq!(
        eval(
            &root,
            "const el = document.body.appendChild(document.createElement('div'));
             const a = el.animate([{ opacity: 0, transform: 'translateX(0px)' }, { opacity: 1, transform: 'translateX(100px)' }],
               { duration: 1000, fill: 'forwards' });
             a.pause(); a.currentTime = 250; a.commitStyles();
             const mid = el.style.opacity + '|' + el.style.transform;
             a.finish(); a.commitStyles();
             mid + ' ' + el.style.opacity + '|' + el.style.transform"
        ),
        "0.25|translateX(25px) 1|translateX(100px)"
    );
}

#[test]
fn shadow_roots_and_documents_list_their_own_animations() {
    let (_t, root) = page();
    assert_eq!(
        eval(
            &root,
            "const host = document.body.appendChild(document.createElement('div'));
             const sr = host.attachShadow({ mode: 'open' });
             const inner = sr.appendChild(document.createElement('span'));
             const a = host.animate(null, 1000), b = inner.animate(null, 1000);
             const ids = list => list.map(x => x === a ? 'a' : x === b ? 'b' : '?').join('');
             [ids(document.getAnimations()), ids(sr.getAnimations()), ids(host.getAnimations()),
              ids(document.body.getAnimations({ subtree: true }))].join()"
        ),
        "a,b,a,a"
    );
}

#[test]
fn transitions_start_after_a_layout_read_or_a_frame_without_get_computed_style() {
    let (mut t, root) = page();
    eval(
        &root,
        "const style = document.head.appendChild(document.createElement('style'));
         style.textContent = '.box { width: 100px; transition: width 2ms } .box.open { width: 200px }';
         globalThis.log = [];
         globalThis.a = document.body.appendChild(document.createElement('div'));
         globalThis.b = document.body.appendChild(document.createElement('div'));
         a.className = b.className = 'box';
         setTimeout(() => {
           a.addEventListener('transitionend', e => log.push('a:' + e.propertyName));
           b.addEventListener('transitionend', e => log.push('b:' + e.propertyName));
           // A forced layout is the before-change style.
           a.offsetHeight;
           a.classList.add('open');
         }, 0); 0",
    );
    run_until(&mut t, &root, "log.includes('a:width')");
    // A change in a later task: the frame after the last change was b's
    // style change event.
    eval(&root, "setTimeout(() => b.classList.add('open'), 30); 0");
    run_until(&mut t, &root, "log.includes('b:width')");
    assert_eq!(eval(&root, "log.join()"), "a:width,b:width");
}

#[test]
fn get_animations_lists_css_animations_and_transitions() {
    let (mut t, root) = page();
    eval(
        &root,
        "const style = document.head.appendChild(document.createElement('style'));
         style.textContent = '@keyframes spin { to { opacity: 0.5 } } .spin { animation: spin 10s 2 }' +
           ' .box { width: 100px; transition: width 10s 1s } .box.open { width: 200px }';
         globalThis.spinner = document.body.appendChild(document.createElement('div'));
         spinner.className = 'spin';
         globalThis.box = document.body.appendChild(document.createElement('div'));
         box.className = 'box';
         0",
    );
    t.run_until_idle(Some(Instant::now() + Duration::from_millis(20)));
    eval(
        &root,
        // The first call keeps CSS animations and transitions from now on.
        "globalThis.first = document.getAnimations();
         // A change in a task: the frame after it starts the transition.
         setTimeout(() => { box.offsetHeight; box.classList.add('open'); }, 0);
         0",
    );
    t.run_until_idle(Some(Instant::now() + Duration::from_millis(60)));
    assert_eq!(
        eval(
            &root,
            "const list = document.getAnimations();
             const anim = list.find(a => a instanceof CSSAnimation), tr = list.find(a => a instanceof CSSTransition);
             [first.length, list.length, list[0] === tr, anim.animationName, anim.playState,
              anim.effect.getTiming().duration, anim.effect.getTiming().iterations, tr.transitionProperty,
              tr.effect.getTiming().delay, document.getAnimations()[1] === anim,
              spinner.getAnimations()[0] === anim, box.getAnimations().length].join()"
        ),
        "1,2,true,spin,running,10000,2,width,1000,true,true,1"
    );
    // A removed animation name leaves the list.
    eval(&root, "setTimeout(() => { spinner.className = ''; }, 0); 0");
    t.run_until_idle(Some(Instant::now() + Duration::from_millis(60)));
    assert_eq!(eval(&root, "document.getAnimations().map(a => a.constructor.name).join()"), "CSSTransition");
}

/// A page with the sheet `css`, listening for the transition events
/// (`log`), with helpers to add a sheet and an element: `sheet(text)`,
/// `el(id, class, parent)`. The sheet is registered in a task of its own:
/// a `<style>` appended and read in one task is not in the cascade yet.
fn transition_page(css: &str) -> (BrowserThread, Realm) {
    let (mut t, root) = page();
    eval(
        &root,
        &format!(
            "globalThis.log = [];
             for (const type of ['transitionrun', 'transitionstart', 'transitionend', 'transitioncancel'])
               addEventListener(type, e => log.push(e.type + ':' + e.propertyName + ':' + e.target.id));
             globalThis.sheet = text => {{ const s = document.createElement('style'); s.textContent = text; document.head.appendChild(s); return s; }};
             globalThis.el = (id, cls, parent = document.body) => {{ const d = document.createElement('div'); d.id = id; if (cls) d.className = cls; parent.appendChild(d); return d; }};
             sheet('{css}'); 0"
        ),
    );
    t.run_until_idle(Some(Instant::now() + Duration::from_millis(50)));
    (t, root)
}

/// `run_until` the log holds `until`, then a quiet 60 ms for any stray event.
fn settle(t: &mut BrowserThread, root: &Realm, until: &str) -> String {
    run_until(t, root, &format!("log.includes('{until}')"));
    t.run_until_idle(Some(Instant::now() + Duration::from_millis(60)));
    eval(root, "log.join(' ')")
}

#[test]
fn a_rule_change_that_makes_an_element_a_candidate_starts_no_transition_but_tracks_it() {
    // Chrome: a sheet that brings an element its transition together with a
    // new width starts nothing (and so does a class doing both at once);
    // the element's style then is its before-change style, so a later
    // change transitions.
    let (mut t, root) = transition_page("#a { width: 100px; height: 10px } .w { width: 300px } .t { transition: width 20ms }");
    eval(
        &root,
        "globalThis.a = el('a'); a.offsetHeight;
         sheet('#a { transition: width 20ms; width: 200px }');
         globalThis.b = el('b'); b.offsetHeight;
         b.className = 't w';
         setTimeout(() => { a.style.width = '300px'; b.classList.remove('w'); }, 30); 0",
    );
    let mut log: Vec<String> = settle(&mut t, &root, "transitionend:width:b").split(' ').map(str::to_owned).collect();
    log.sort();
    assert_eq!(
        log,
        [
            "transitionend:width:a",
            "transitionend:width:b",
            "transitionrun:width:a",
            "transitionrun:width:b",
            "transitionstart:width:a",
            "transitionstart:width:b"
        ]
    );
}

#[test]
fn an_element_inserted_with_a_transition_transitions_only_after_a_style_change_event() {
    // Inserted and changed in one task: nothing (no before-change style).
    // Inserted, read (a style change event), changed: a transition; so does
    // a change in a later task, the frame after the insertion being its
    // style change event.
    let (mut t, root) = transition_page(".t { transition: width 20ms; width: 100px; height: 10px } .w { width: 200px }");
    eval(
        &root,
        "const b1 = el('b1', 't'); b1.classList.add('w');
         const b2 = el('b2', 't'); b2.offsetHeight; b2.classList.add('w');
         const b3 = el('b3', 't');
         setTimeout(() => b3.classList.add('w'), 30); 0",
    );
    let mut log: Vec<String> = settle(&mut t, &root, "transitionend:width:b3").split(' ').map(str::to_owned).collect();
    log.sort();
    assert_eq!(
        log,
        [
            "transitionend:width:b2",
            "transitionend:width:b3",
            "transitionrun:width:b2",
            "transitionrun:width:b3",
            "transitionstart:width:b2",
            "transitionstart:width:b3"
        ]
    );
}

#[test]
fn a_style_change_on_an_element_without_a_transition_fires_nothing_until_it_gets_one() {
    // The non-candidate's change is journaled by the engine and skipped by
    // the flush; a sibling that may transition but did not change fires
    // nothing. Once a class gives it a transition (a snapshot, no event), the
    // next change transitions.
    let (mut t, root) = transition_page(".n { width: 100px; height: 10px } .w { width: 200px } .t { transition: width 20ms }");
    eval(
        &root,
        "globalThis.c = el('c', 'n'); el('ct', 't'); c.offsetHeight;
         c.classList.add('w');
         setTimeout(() => { c.offsetHeight; c.classList.add('t'); }, 30);
         setTimeout(() => { c.offsetHeight; c.classList.remove('w'); }, 60); 0",
    );
    assert_eq!(
        settle(&mut t, &root, "transitionend:width:c"),
        "transitionrun:width:c transitionstart:width:c transitionend:width:c"
    );
}

#[test]
fn a_candidate_under_display_none_does_not_transition() {
    // Not rendered: a change while hidden starts nothing, being shown with
    // the new width starts nothing (the engine does not style the element
    // while hidden, and styles it afresh when shown), and neither does
    // hiding together with a change; Chrome starts none of these. Once
    // shown, the next change transitions.
    let (mut t, root) =
        transition_page(".t { transition: width 20ms; width: 100px; height: 10px } .w { width: 200px } .h { display: none }");
    eval(
        &root,
        "const p1 = el('p1', 'h'); const d1 = el('d1', 't', p1); d1.offsetHeight; d1.classList.add('w');
         const p2 = el('p2', 'h'); globalThis.d2 = el('d2', 't', p2); d2.offsetHeight; d2.classList.add('w'); d2.offsetHeight;
         const p3 = el('p3'); const d3 = el('d3', 't', p3); d3.offsetHeight; p3.classList.add('h'); d3.classList.add('w');
         setTimeout(() => { p2.classList.remove('h'); }, 30);
         setTimeout(() => { d2.offsetHeight; d2.classList.remove('w'); }, 60); 0",
    );
    assert_eq!(
        settle(&mut t, &root, "transitionend:width:d2"),
        "transitionrun:width:d2 transitionstart:width:d2 transitionend:width:d2"
    );
}

// Review (tflush): adversarial cases around the style-change journal and the
// `display: none` change. Expectations are Chrome's (headless, checked by
// the review harness).

const TRANSITION_CSS: &str = ".t { transition: width 20ms; width: 100px; height: 10px } .w { width: 200px } .h { display: none }";

/// Runs the page for `ms` and answers the log.
fn quiet(t: &mut BrowserThread, root: &Realm, ms: u64) -> String {
    let deadline = Instant::now() + Duration::from_millis(ms);
    while Instant::now() < deadline {
        t.run_until_idle(Some(Instant::now() + Duration::from_millis(20)));
        std::thread::sleep(Duration::from_millis(2));
    }
    eval(root, "log.join(' ')")
}

#[test]
fn review_a_hidden_element_changed_and_shown_in_one_task_transitions() {
    // Displayed at both style change events: the hidden state in between
    // never reached a style flush.
    let (mut t, root) = transition_page(TRANSITION_CSS);
    eval(&root, "globalThis.a = el('a', 't'); a.offsetHeight; 0");
    t.run_until_idle(Some(Instant::now() + Duration::from_millis(30)));
    eval(&root, "a.style.display = 'none'; a.classList.add('w'); a.style.display = ''; 0");
    assert_eq!(
        settle(&mut t, &root, "transitionend:width:a"),
        "transitionrun:width:a transitionstart:width:a transitionend:width:a"
    );
}

#[test]
fn review_an_element_hidden_at_a_flush_then_changed_and_shown_does_not_transition() {
    let (mut t, root) = transition_page(TRANSITION_CSS);
    eval(&root, "globalThis.a = el('a', 't'); globalThis.p = el('p'); globalThis.b = el('b', 't', p); a.offsetHeight; 0");
    t.run_until_idle(Some(Instant::now() + Duration::from_millis(30)));
    eval(&root, "a.style.display = 'none'; p.hidden = true; getComputedStyle(a).width; b.offsetHeight; 0");
    t.run_until_idle(Some(Instant::now() + Duration::from_millis(30)));
    eval(&root, "a.classList.add('w'); b.classList.add('w'); a.offsetHeight; 0");
    t.run_until_idle(Some(Instant::now() + Duration::from_millis(30)));
    eval(&root, "a.style.display = ''; p.hidden = false; 0");
    assert_eq!(quiet(&mut t, &root, 150), "");
}

#[test]
fn review_hidden_ancestor_shown_then_read_then_changed_transitions() {
    let (mut t, root) = transition_page(TRANSITION_CSS);
    eval(&root, "globalThis.p = el('p', 'h'); globalThis.a = el('a', 't', p); a.offsetHeight; 0");
    t.run_until_idle(Some(Instant::now() + Duration::from_millis(30)));
    eval(&root, "p.classList.remove('h'); getComputedStyle(a).width; a.classList.add('w'); 0");
    assert_eq!(
        settle(&mut t, &root, "transitionend:width:a"),
        "transitionrun:width:a transitionstart:width:a transitionend:width:a"
    );
}

#[test]
fn review_inherited_custom_property_and_structural_selectors_reach_the_journal() {
    // Restyles the traversal reaches through an ancestor or a sibling.
    let (mut t, root) = transition_page(
        ".t { transition: width 20ms; width: var(--w, 100px); height: 10px } .on ~ .t { width: 150px } .t:has(.in) { width: 170px }",
    );
    eval(
        &root,
        "globalThis.a = el('a', 't'); globalThis.s = el('s'); globalThis.b = el('b', 't'); globalThis.c = el('c', 't'); globalThis.k = el('k', '', c); a.offsetHeight; 0",
    );
    t.run_until_idle(Some(Instant::now() + Duration::from_millis(30)));
    eval(&root, "document.body.style.setProperty('--w', '120px'); s.className = 'on'; k.className = 'in'; 0");
    let mut log: Vec<String> = settle(&mut t, &root, "transitionend:width:c").split(' ').map(str::to_owned).collect();
    run_until(&mut t, &root, "log.includes('transitionend:width:a') && log.includes('transitionend:width:b')");
    log = eval(&root, "log.join(' ')").split(' ').map(str::to_owned).collect();
    log.sort();
    assert_eq!(log.len(), 9, "{log:?}");
    for id in ["a", "b", "c"] {
        assert!(log.contains(&format!("transitionrun:width:{id}")), "{log:?}");
    }
}

#[test]
fn review_many_style_flushes_in_one_task_keep_the_journal() {
    // Each flush takes the journal; the element changed at the end still
    // transitions, and only once.
    let (mut t, root) = transition_page(".t { transition: width 20ms; width: 100px; height: 10px } .w { width: 200px } .k span { color: red }");
    eval(&root, "for (let i = 0; i < 300; i++) el('s' + i, ''); globalThis.a = el('a', 't'); a.offsetHeight; 0");
    t.run_until_idle(Some(Instant::now() + Duration::from_millis(30)));
    eval(
        &root,
        "for (let i = 0; i < 20; i++) { document.body.classList.toggle('k'); document.body.offsetHeight; } a.classList.add('w'); 0",
    );
    assert_eq!(
        settle(&mut t, &root, "transitionend:width:a"),
        "transitionrun:width:a transitionstart:width:a transitionend:width:a"
    );
}

#[test]
fn review_a_journal_overflow_falls_back_to_every_candidate() {
    // One traversal restyles more than 65,536 elements: the journal is
    // dropped, the flush looks at every candidate, and the next flush is
    // journaled again.
    let (mut t, root) =
        transition_page(".t { transition: width 20ms; width: 100px; height: 10px } .w { width: 200px } .k span { color: red }");
    eval(
        &root,
        "const f = document.createDocumentFragment(); for (let i = 0; i < 66000; i++) f.appendChild(document.createElement('span'));
         document.body.appendChild(f); globalThis.a = el('a', 't'); a.offsetHeight; 0",
    );
    t.run_until_idle(Some(Instant::now() + Duration::from_millis(30)));
    eval(&root, "document.body.classList.toggle('k'); a.classList.add('w'); a.offsetHeight; 0");
    settle(&mut t, &root, "transitionend:width:a");
    eval(&root, "a.classList.remove('w'); a.offsetHeight; 0");
    run_until(&mut t, &root, "log.filter(e => e == 'transitionend:width:a').length == 2");
    assert_eq!(eval(&root, "log.length"), "6");
}

#[test]
fn review_an_iframe_candidate_under_display_none_does_not_transition() {
    let (mut t, root) = transition_page(TRANSITION_CSS);
    eval(
        &root,
        &format!(
            "globalThis.f = el('f'); f.remove(); globalThis.f = document.createElement('iframe');
             f.srcdoc = '<style>{TRANSITION_CSS}</style><div id=p class=h><div id=i class=t></div></div><div id=j class=t></div>';
             f.onload = () => {{ for (const type of ['transitionrun', 'transitionstart', 'transitionend', 'transitioncancel'])
               f.contentWindow.addEventListener(type, e => log.push(e.type + ':' + e.propertyName + ':' + e.target.id));
               globalThis.loaded = true; }};
             document.body.appendChild(f); 0"
        ),
    );
    run_until(&mut t, &root, "globalThis.loaded === true");
    eval(&root, "{ const d = f.contentDocument; d.getElementById('i').offsetHeight; d.getElementById('i').classList.add('w'); d.getElementById('i').offsetHeight; } 0");
    t.run_until_idle(Some(Instant::now() + Duration::from_millis(30)));
    eval(&root, "{ const d = f.contentDocument; d.getElementById('p').classList.remove('h'); d.getElementById('j').classList.add('w'); } 0");
    assert_eq!(
        settle(&mut t, &root, "transitionend:width:j"),
        "transitionrun:width:j transitionstart:width:j transitionend:width:j"
    );
}

#[test]
fn review_a_document_transition_rule_does_not_reach_into_a_shadow_tree() {
    // Chrome: a document rule does not style shadow-tree elements, so a
    // width change of one there never transitions (base: none either).
    // FAILS on b3bc5b71: the journal lists the shadow element and
    // `may_transition` / `TimingRules::element` match the document's `.t`
    // rule against it across the scope boundary.
    let (mut t, root) = transition_page(TRANSITION_CSS);
    eval(
        &root,
        "globalThis.host = el('host'); const r = host.attachShadow({ mode: 'open' });
         for (const type of ['transitionrun', 'transitionstart', 'transitionend', 'transitioncancel'])
           r.addEventListener(type, e => log.push(e.type + ':' + e.propertyName + ':' + e.target.id));
         r.innerHTML = '<style>div { height: 10px; width: 100px } .w { width: 200px }</style><div id=s class=t></div>';
         globalThis.s = r.getElementById('s'); s.offsetHeight; 0",
    );
    t.run_until_idle(Some(Instant::now() + Duration::from_millis(30)));
    eval(&root, "s.classList.add('w'); s.offsetHeight; 0");
    t.run_until_idle(Some(Instant::now() + Duration::from_millis(60)));
    eval(&root, "s.classList.remove('w'); s.offsetHeight; 0");
    assert_eq!(quiet(&mut t, &root, 150), "");
}

#[test]
fn review_closed_details_content_transitions_when_opened() {
    // Chrome keeps the style of a closed `<details>`'s content (it is
    // content-visibility: hidden, not display: none there): a change while
    // closed transitions once opened (base: the same). FAILS on b3bc5b71:
    // the engine styles the content `display: none`, so its before-change
    // style is dropped.
    let (mut t, root) = transition_page(TRANSITION_CSS);
    eval(
        &root,
        "globalThis.d = document.createElement('details'); document.body.appendChild(d); globalThis.a = el('a', 't', d); a.offsetHeight; 0",
    );
    t.run_until_idle(Some(Instant::now() + Duration::from_millis(30)));
    eval(&root, "a.classList.add('w'); a.offsetHeight; 0");
    t.run_until_idle(Some(Instant::now() + Duration::from_millis(30)));
    eval(&root, "d.open = true; 0");
    assert_eq!(
        settle(&mut t, &root, "transitionend:width:a"),
        "transitionrun:width:a transitionstart:width:a transitionend:width:a"
    );
}

#[test]
fn review_hidden_until_found_content_transitions_when_revealed() {
    // As closed `<details>`: Chrome (and base) transition. FAILS on b3bc5b71.
    let (mut t, root) = transition_page(TRANSITION_CSS);
    eval(
        &root,
        "globalThis.p = el('p'); p.setAttribute('hidden', 'until-found'); globalThis.a = el('a', 't', p); a.offsetHeight; 0",
    );
    t.run_until_idle(Some(Instant::now() + Duration::from_millis(30)));
    eval(&root, "a.classList.add('w'); a.offsetHeight; 0");
    t.run_until_idle(Some(Instant::now() + Duration::from_millis(30)));
    eval(&root, "p.removeAttribute('hidden'); 0");
    assert_eq!(
        settle(&mut t, &root, "transitionend:width:a"),
        "transitionrun:width:a transitionstart:width:a transitionend:width:a"
    );
}

#[test]
fn review_a_reversed_transition_cancels_and_runs_again() {
    let (mut t, root) = transition_page(".t { transition: width 300ms; width: 100px; height: 10px } .w { width: 200px }");
    eval(&root, "globalThis.a = el('a', 't'); a.offsetHeight; 0");
    t.run_until_idle(Some(Instant::now() + Duration::from_millis(30)));
    eval(&root, "a.classList.add('w'); a.offsetHeight; 0");
    run_until(&mut t, &root, "log.includes('transitionstart:width:a')");
    eval(&root, "a.classList.remove('w'); a.offsetHeight; 0");
    run_until(&mut t, &root, "log.filter(e => e == 'transitionstart:width:a').length == 2");
    assert_eq!(
        eval(&root, "log.slice(0, 4).join(' ')"),
        "transitionrun:width:a transitionstart:width:a transitioncancel:width:a transitionrun:width:a"
    );
}

#[test]
fn review_a_sheet_added_after_the_journal_started_transitions_its_targets() {
    let (mut t, root) = transition_page(TRANSITION_CSS);
    eval(&root, "globalThis.a = el('a', 't'); globalThis.b = el('b'); b.style.cssText = 'width: 100px; height: 10px'; a.offsetHeight; 0");
    t.run_until_idle(Some(Instant::now() + Duration::from_millis(30)));
    eval(&root, "a.classList.add('w'); a.offsetHeight; 0");
    settle(&mut t, &root, "transitionend:width:a");
    // `b` becomes a candidate by a new sheet (a full scan), then changes.
    eval(&root, "sheet('#b { transition: width 20ms }'); 0");
    t.run_until_idle(Some(Instant::now() + Duration::from_millis(30)));
    eval(&root, "b.offsetHeight; b.style.width = '150px'; 0");
    run_until(&mut t, &root, "log.includes('transitionend:width:b')");
    assert_eq!(eval(&root, "log.filter(e => e.endsWith(':b')).join(' ')"), "transitionrun:width:b transitionstart:width:b transitionend:width:b");
}
