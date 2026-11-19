//! Web Animations: `Element.animate`, `Animation`, `KeyframeEffect`,
//! `document.timeline` and `getAnimations`.

use pyro_browser::*;
use pyro_js::Realm;
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
fn a_descendant_restyled_through_its_ancestor_transitions_without_being_read() {
    // The elements whose style changed come from the style traversal, not
    // from reading them: a class on the ancestor changes the box's width
    // through `.open .box`, and the next flush starts its transition; so does
    // a logical property (`inline-size`, mapped to `width` to be animated). A
    // width that was never a length (`auto`) transitions nothing.
    let (mut t, root) = page();
    eval(
        &root,
        "const style = document.head.appendChild(document.createElement('style'));
         style.textContent = '.box { width: 100px; transition: width 2ms } .open .box { width: 200px } .open .sized { width: 50px; transition: width 2ms } .logical { inline-size: 100px; transition: inline-size 2ms } .open .logical { inline-size: 200px }';
         globalThis.log = [];
         globalThis.wrap = document.body.appendChild(document.createElement('div'));
         globalThis.box = wrap.appendChild(document.createElement('div'));
         box.className = 'box';
         globalThis.sized = wrap.appendChild(document.createElement('div'));
         sized.className = 'sized';
         globalThis.logical = wrap.appendChild(document.createElement('div'));
         logical.className = 'logical';
         addEventListener('transitionend', e => log.push(e.target.className + ':' + e.propertyName));
         setTimeout(() => {
           // A flush that reads nothing of the boxes: their first styles.
           document.body.offsetHeight;
           wrap.className = 'open';
           document.body.offsetHeight;
         }, 0); 0",
    );
    run_until(&mut t, &root, "log.length > 1");
    t.run_until_idle(Some(Instant::now() + Duration::from_millis(30)));
    assert_eq!(eval(&root, "log.sort().join()"), "box:width,logical:inline-size");
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
fn a_rule_change_that_makes_an_element_a_candidate_transitions_and_tracks_it() {
    // Chrome (checked 2026-11-18): a sheet that brings an element its transition together with a
    // new width transitions it (the after-change style's `transition-*` applies, css-transitions-1
    // §3), and a later change transitions again. `b`'s class brings its transition and width
    // auto -> 300px, then 300px -> auto: no browser interpolates `auto`, so it starts nothing
    // (`changed_longhands` asks Stylo to animate between the values, as since #283).
    let (mut t, root) =
        transition_page("#a { width: 100px; height: 10px } .w { width: 300px } .t { transition: width 20ms }");
    eval(
        &root,
        "globalThis.a = el('a'); a.offsetHeight;
         sheet('#a { transition: width 20ms; width: 200px }');
         globalThis.b = el('b'); b.offsetHeight;
         b.className = 't w';
         setTimeout(() => { a.style.width = '300px'; b.classList.remove('w'); }, 30); 0",
    );
    run_until(&mut t, &root, "log.filter(e => e == 'transitionend:width:a').length == 2");
    t.run_until_idle(Some(Instant::now() + Duration::from_millis(60)));
    let mut log: Vec<String> = eval(&root, "log.join(' ')").split(' ').map(str::to_owned).collect();
    log.sort();
    assert_eq!(
        log,
        [
            "transitionend:width:a",
            "transitionend:width:a",
            "transitionrun:width:a",
            "transitionrun:width:a",
            "transitionstart:width:a",
            "transitionstart:width:a"
        ]
    );
}

#[test]
fn an_element_inserted_with_a_transition_transitions_only_after_a_style_change_event() {
    // Inserted and changed in one task: nothing (no before-change style).
    // Inserted, read (a style change event), changed: a transition; so does
    // a change in a later task, the frame after the insertion being its
    // style change event.
    let (mut t, root) =
        transition_page(".t { transition: width 20ms; width: 100px; height: 10px } .w { width: 200px }");
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
    let (mut t, root) =
        transition_page(".n { width: 100px; height: 10px } .w { width: 200px } .t { transition: width 20ms }");
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
    let (mut t, root) = transition_page(
        ".t { transition: width 20ms; width: 100px; height: 10px } .w { width: 200px } .h { display: none }",
    );
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

const TRANSITION_CSS: &str =
    ".t { transition: width 20ms; width: 100px; height: 10px } .w { width: 200px } .h { display: none }";

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
    eval(
        &root,
        "globalThis.a = el('a', 't'); globalThis.p = el('p'); globalThis.b = el('b', 't', p); a.offsetHeight; 0",
    );
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
    settle(&mut t, &root, "transitionend:width:c");
    run_until(&mut t, &root, "log.includes('transitionend:width:a') && log.includes('transitionend:width:b')");
    let mut log: Vec<String> = eval(&root, "log.join(' ')").split(' ').map(str::to_owned).collect();
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
    let (mut t, root) = transition_page(
        ".t { transition: width 20ms; width: 100px; height: 10px } .w { width: 200px } .k span { color: red }",
    );
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
    let (mut t, root) = transition_page(
        ".t { transition: width 20ms; width: 100px; height: 10px } .w { width: 200px } .k span { color: red }",
    );
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
    // `p` is `display: none` here from its first style, which the engine
    // does not record as a change, and the engine never styles `a` under
    // it. Chrome styles `a` when script reads it (`p` is
    // content-visibility: hidden there); the read here keeps that style as
    // `a`'s before-change style, so the change made while hidden
    // transitions once `p` is shown. As closed `<details>`: Chrome (and
    // 34f0f2ba) transition.
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
    let (mut t, root) =
        transition_page(".t { transition: width 300ms; width: 100px; height: 10px } .w { width: 200px }");
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
    assert_eq!(
        eval(&root, "log.filter(e => e.endsWith(':b')).join(' ')"),
        "transitionrun:width:b transitionstart:width:b transitionend:width:b"
    );
}

#[test]
fn content_of_a_details_closed_after_its_first_style_keeps_it() {
    // Chrome: a nested element styled while open, changed while closed,
    // transitions once opened again (the closed content keeps its style).
    let (mut t, root) = transition_page(TRANSITION_CSS);
    eval(
        &root,
        "globalThis.d = document.createElement('details'); d.open = true; document.body.appendChild(d);
         globalThis.w = el('w', '', d); globalThis.a = el('a', 't', w); a.offsetHeight; 0",
    );
    t.run_until_idle(Some(Instant::now() + Duration::from_millis(30)));
    eval(&root, "d.open = false; a.offsetHeight; 0");
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
fn closed_details_content_under_display_none_does_not_transition() {
    // Chrome: the `display: none` ancestor at a flush drops the style, even
    // for content a closed `<details>` hides.
    let (mut t, root) = transition_page(TRANSITION_CSS);
    eval(
        &root,
        "globalThis.q = el('q'); globalThis.d = document.createElement('details'); q.appendChild(d);
         globalThis.a = el('a', 't', d); a.offsetHeight; 0",
    );
    t.run_until_idle(Some(Instant::now() + Duration::from_millis(30)));
    eval(&root, "q.style.display = 'none'; a.offsetHeight; 0");
    t.run_until_idle(Some(Instant::now() + Duration::from_millis(30)));
    eval(&root, "a.classList.add('w'); a.offsetHeight; 0");
    t.run_until_idle(Some(Instant::now() + Duration::from_millis(30)));
    eval(&root, "q.style.display = ''; d.open = true; 0");
    assert_eq!(quiet(&mut t, &root, 150), "");
}

#[test]
fn an_element_changed_together_with_its_own_display_none_does_not_transition() {
    // Chrome: an element `display: none` at either style change event is not
    // rendered there, so a width changed together with hiding it, or with
    // showing it again, starts nothing (#283 started one: the engine keeps a
    // `display: none` element's own style, so it was a replacement).
    let (mut t, root) = transition_page(TRANSITION_CSS);
    eval(&root, "globalThis.a = el('a', 't'); a.offsetHeight; 0");
    t.run_until_idle(Some(Instant::now() + Duration::from_millis(30)));
    eval(&root, "a.classList.add('w', 'h'); a.offsetHeight; 0");
    t.run_until_idle(Some(Instant::now() + Duration::from_millis(30)));
    eval(&root, "a.classList.remove('w', 'h'); a.offsetHeight; 0");
    assert_eq!(quiet(&mut t, &root, 150), "");
    // Rendered at both: transitions.
    eval(&root, "a.classList.add('w'); a.offsetHeight; 0");
    assert_eq!(
        settle(&mut t, &root, "transitionend:width:a"),
        "transitionrun:width:a transitionstart:width:a transitionend:width:a"
    );
}

#[test]
fn hidden_until_found_content_never_read_before_it_is_revealed_does_not_transition() {
    // Chrome: content-visibility: hidden content no script read has no style
    // until it is shown; its first style is no change.
    let (mut t, root) = transition_page(TRANSITION_CSS);
    eval(
        &root,
        "globalThis.p = el('p'); p.setAttribute('hidden', 'until-found'); globalThis.a = el('a', 't', p); p.offsetHeight; 0",
    );
    t.run_until_idle(Some(Instant::now() + Duration::from_millis(30)));
    eval(&root, "a.classList.add('w'); p.offsetHeight; 0");
    t.run_until_idle(Some(Instant::now() + Duration::from_millis(30)));
    eval(&root, "p.removeAttribute('hidden'); 0");
    assert_eq!(quiet(&mut t, &root, 150), "");
}

#[test]
fn nested_content_of_a_details_closed_from_its_first_style_transitions_when_read_and_opened() {
    // As `hidden=until-found`: the read styles the closed details' content
    // (Chrome), so a change made while closed transitions once opened. `w`
    // is the content root (the details' child), `a` an element under it.
    let (mut t, root) = transition_page(TRANSITION_CSS);
    eval(
        &root,
        "globalThis.d = document.createElement('details'); document.body.appendChild(d);
         globalThis.w = el('w', '', d); globalThis.a = el('a', 't', w); a.getBoundingClientRect(); 0",
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
fn hidden_until_found_content_read_under_display_none_does_not_transition() {
    // A real `display: none` above the until-found root: the read styles
    // nothing for transitions (Chrome: not rendered).
    let (mut t, root) = transition_page(TRANSITION_CSS);
    eval(
        &root,
        "globalThis.q = el('q', 'h'); globalThis.p = el('p', '', q); p.setAttribute('hidden', 'until-found');
         globalThis.a = el('a', 't', p); a.offsetHeight; 0",
    );
    t.run_until_idle(Some(Instant::now() + Duration::from_millis(30)));
    eval(&root, "a.classList.add('w'); a.offsetHeight; 0");
    t.run_until_idle(Some(Instant::now() + Duration::from_millis(30)));
    eval(&root, "q.classList.remove('h'); p.removeAttribute('hidden'); 0");
    assert_eq!(quiet(&mut t, &root, 150), "");
}

// ---------------- review (misc2): hidden content styled by a read (`style_read_hidden_content`).
// Expected logs are Chrome 142 headless, from the same steps in a page (each `eval` a task). ----------------

/// Runs `steps` as separate tasks (30 ms apart), then answers the log after 150 ms.
fn misc2_steps(steps: &[&str]) -> String {
    let (mut t, root) = transition_page(TRANSITION_CSS);
    for step in steps {
        eval(&root, &format!("{step}; 0"));
        t.run_until_idle(Some(Instant::now() + Duration::from_millis(30)));
    }
    quiet(&mut t, &root, 150)
}

const MISC2_RAN: &str = "transitionrun:width:a transitionstart:width:a transitionend:width:a";

#[test]
fn review_misc2_until_found_content_read_by_computed_style_transitions() {
    assert_eq!(
        misc2_steps(&[
            "globalThis.p = el('p'); p.setAttribute('hidden', 'until-found'); globalThis.a = el('a', 't', p); getComputedStyle(a).width",
            "a.classList.add('w'); getComputedStyle(a).width",
            "p.removeAttribute('hidden')",
        ]),
        MISC2_RAN
    );
}

#[test]
fn review_misc2_until_found_content_read_by_bounding_rect_then_shown_by_hidden_false_transitions() {
    assert_eq!(
        misc2_steps(&[
            "globalThis.p = el('p'); p.setAttribute('hidden', 'until-found'); globalThis.a = el('a', 't', p); a.getBoundingClientRect()",
            "a.classList.add('w')",
            "p.hidden = false",
        ]),
        MISC2_RAN
    );
}

#[test]
fn review_misc2_a_read_of_a_sibling_styles_the_whole_until_found_content() {
    assert_eq!(
        misc2_steps(&[
            "globalThis.p = el('p'); p.setAttribute('hidden', 'until-found'); globalThis.a = el('a', 't', p); globalThis.b = el('b', '', p); b.offsetHeight",
            "a.classList.add('w')",
            "p.removeAttribute('hidden')",
        ]),
        MISC2_RAN
    );
}

#[test]
fn review_misc2_a_second_read_after_the_change_keeps_the_first_style() {
    // Chrome: the style of the first read stays the before-change style.
    assert_eq!(
        misc2_steps(&[
            "globalThis.p = el('p'); p.setAttribute('hidden', 'until-found'); globalThis.a = el('a', 't', p); a.offsetHeight",
            "a.classList.add('w')",
            "a.offsetHeight",
            "p.removeAttribute('hidden')",
        ]),
        MISC2_RAN
    );
}

#[test]
fn review_misc2_a_first_read_after_the_change_does_not_transition() {
    assert_eq!(
        misc2_steps(&[
            "globalThis.p = el('p'); p.setAttribute('hidden', 'until-found'); globalThis.a = el('a', 't', p)",
            "a.classList.add('w'); a.offsetHeight",
            "p.removeAttribute('hidden')",
        ]),
        ""
    );
}

#[test]
fn review_misc2_content_under_a_real_display_none_inside_until_found_does_not_transition() {
    // Chrome: `a` stays under `q`'s display: none after the reveal, so it is not rendered and
    // does not transition. FAILS on 3a80cd0f (base 267ac8c5: none): `reveal_hidden_content`
    // checks only `a`'s own display, not the `display: none` between it and the root.
    assert_eq!(
        misc2_steps(&[
            "globalThis.p = el('p'); p.setAttribute('hidden', 'until-found'); globalThis.q = el('q', 'h', p); globalThis.a = el('a', 't', q); a.offsetHeight",
            "a.classList.add('w'); a.offsetHeight",
            "p.removeAttribute('hidden')",
        ]),
        ""
    );
}

#[test]
fn review_misc2_an_until_found_root_removed_and_reinserted_does_not_transition() {
    // Chrome: a removed subtree loses its styles; reinserted and revealed, `a` gets a first
    // style, no transition. FAILS on 3a80cd0f (base: none): `hidden_roots`/`hidden_content`
    // keep the root and the read's snapshot across the removal.
    assert_eq!(
        misc2_steps(&[
            "globalThis.p = el('p'); p.setAttribute('hidden', 'until-found'); globalThis.a = el('a', 't', p); a.offsetHeight",
            "p.remove(); a.classList.add('w')",
            "document.body.appendChild(p)",
            "p.removeAttribute('hidden')",
        ]),
        ""
    );
}

#[test]
fn review_misc2_an_until_found_root_removed_reinserted_and_read_again_does_not_transition() {
    // Chrome: the read after reinsertion is `a`'s first style (width 200px). FAILS on 3a80cd0f
    // (base: none): the snapshot from before the removal (100px) is kept and wins.
    assert_eq!(
        misc2_steps(&[
            "globalThis.p = el('p'); p.setAttribute('hidden', 'until-found'); globalThis.a = el('a', 't', p); a.offsetHeight",
            "p.remove(); a.classList.add('w')",
            "document.body.appendChild(p); a.offsetHeight",
            "p.removeAttribute('hidden')",
        ]),
        ""
    );
}

#[test]
fn review_misc2_content_moved_out_of_an_until_found_root_does_not_transition() {
    assert_eq!(
        misc2_steps(&[
            "globalThis.p = el('p'); p.setAttribute('hidden', 'until-found'); globalThis.a = el('a', 't', p); a.offsetHeight",
            "a.classList.add('w')",
            "document.body.appendChild(a)",
        ]),
        ""
    );
}

#[test]
fn review_misc2_nested_until_found_roots_revealed_outer_then_inner_transition() {
    assert_eq!(
        misc2_steps(&[
            "globalThis.o = el('o'); o.setAttribute('hidden', 'until-found'); globalThis.p = el('p', '', o); p.setAttribute('hidden', 'until-found'); globalThis.a = el('a', 't', p); a.offsetHeight",
            "a.classList.add('w')",
            "o.removeAttribute('hidden')",
            "p.removeAttribute('hidden')",
        ]),
        MISC2_RAN
    );
}

#[test]
fn review_misc2_until_found_in_a_closed_details_opened_then_revealed_transitions() {
    assert_eq!(
        misc2_steps(&[
            "globalThis.d = document.createElement('details'); document.body.appendChild(d); globalThis.p = el('p', '', d); p.setAttribute('hidden', 'until-found'); globalThis.a = el('a', 't', p); a.offsetHeight",
            "a.classList.add('w')",
            "d.open = true",
            "p.removeAttribute('hidden')",
        ]),
        MISC2_RAN
    );
}

#[test]
fn review_misc2_a_closed_details_in_until_found_revealed_together_transitions() {
    assert_eq!(
        misc2_steps(&[
            "globalThis.p = el('p'); p.setAttribute('hidden', 'until-found'); globalThis.d = document.createElement('details'); p.appendChild(d); globalThis.a = el('a', 't', d); a.offsetHeight",
            "a.classList.add('w')",
            "d.open = true; p.removeAttribute('hidden')",
        ]),
        MISC2_RAN
    );
}

#[test]
#[ignore = "review misc2: FAILS on 3a80cd0f and on base 267ac8c5 (no transition)"]
fn review_misc2_a_closed_details_in_until_found_revealed_outer_first_then_opened_transitions() {
    // Chrome: `a` (the details' content) keeps the style of the read while the details stays
    // closed after the outer reveal. Here the outer reveal drops it (`reveal_hidden_content`
    // finds `a` still `display: none`), and `a` is the inner content root, whose own
    // before-change style is the engine's (200px, styled at the outer reveal).
    assert_eq!(
        misc2_steps(&[
            "globalThis.p = el('p'); p.setAttribute('hidden', 'until-found'); globalThis.d = document.createElement('details'); p.appendChild(d); globalThis.a = el('a', 't', d); a.offsetHeight",
            "a.classList.add('w')",
            "p.removeAttribute('hidden')",
            "d.open = true",
        ]),
        MISC2_RAN
    );
}

#[test]
#[ignore = "review misc2: FAILS on 3a80cd0f and on base 267ac8c5 (transitions)"]
fn review_misc2_closed_details_content_never_read_does_not_transition() {
    // Chrome: content no script read has no style before the details opens. Here the content
    // root (the details' child) is styled by the engine (`display: none` root of its subtree),
    // and its own change is a style change.
    assert_eq!(
        misc2_steps(&[
            "globalThis.d = document.createElement('details'); document.body.appendChild(d); globalThis.a = el('a', 't', d)",
            "a.classList.add('w')",
            "d.open = true",
        ]),
        ""
    );
}

#[test]
#[ignore = "review misc2: FAILS on 3a80cd0f and on base 267ac8c5 (no beforematch, no reveal)"]
fn review_misc2_fragment_navigation_reveals_until_found_with_beforematch() {
    // Chrome: `location.hash = '#a'` fires `beforematch` at `p`, removes its `hidden` and the read
    // style transitions.
    let (mut t, root) = transition_page(TRANSITION_CSS);
    eval(&root, "addEventListener('beforematch', e => log.push('beforematch:' + e.target.id), true); 0");
    for step in [
        "globalThis.p = el('p'); p.setAttribute('hidden', 'until-found'); globalThis.a = el('a', 't', p); a.offsetHeight",
        "a.classList.add('w')",
        "location.hash = '#a'",
    ] {
        eval(&root, &format!("{step}; 0"));
        t.run_until_idle(Some(Instant::now() + Duration::from_millis(30)));
    }
    assert_eq!(quiet(&mut t, &root, 150), format!("beforematch:p {MISC2_RAN}"));
}

/// Animated values are in the cascade: computed styles read a CSS
/// animation (paused, a negative delay putting it halfway) and a script
/// animation where they are, and the base value once they are gone.
#[test]
fn computed_styles_read_css_and_script_animations() {
    let (mut t, root) = page();
    eval(
        &root,
        "const style = document.head.appendChild(document.createElement('style'));
         style.textContent = '@keyframes fade { from { opacity: 0 } to { opacity: 1 } }' +
           ' .fade { animation: fade 10s linear -5s paused }';
         globalThis.a = document.body.appendChild(document.createElement('div'));
         globalThis.b = document.body.appendChild(document.createElement('div'));
         0",
    );
    t.run_until_idle(Some(Instant::now() + Duration::from_millis(30)));
    assert_eq!(
        eval(
            &root,
            // Read in the task that set the class: the flush starts it.
            "a.className = 'fade';
             const css = getComputedStyle(a).opacity;
             globalThis.anim = b.animate([{ opacity: 0.2 }, { opacity: 0.6 }], { duration: 1000 });
             anim.pause();
             anim.currentTime = 250;
             [css, getComputedStyle(b).opacity].join()"
        ),
        "0.5,0.3"
    );
    assert_eq!(
        eval(
            &root,
            "anim.cancel(); a.className = ''; [getComputedStyle(a).opacity, getComputedStyle(b).opacity].join()"
        ),
        "1,1"
    );
}

/// A transition whose element was never read before its style changed is
/// still a transition (the frames after the element appeared styled it), and
/// `getAnimations()` in the same task lists it: the reftests that wait on
/// `document.getAnimations()[0]` right after a change.
#[test]
fn get_animations_lists_a_transition_started_in_the_same_task() {
    let (mut t, root) = page();
    eval(
        &root,
        "const style = document.head.appendChild(document.createElement('style'));
         style.textContent = '.box { background-color: rgb(0, 200, 0); transition: background-color 1000s steps(2) -499.9s }';
         globalThis.box = document.body.appendChild(document.createElement('div'));
         box.className = 'box';
         0",
    );
    t.run_until_idle(Some(Instant::now() + Duration::from_millis(50)));
    assert_eq!(
        eval(
            &root,
            "box.style.backgroundColor = 'rgb(200, 0, 0)';
             const list = document.getAnimations();
             [list.length, list[0] instanceof CSSTransition, list[0].transitionProperty,
              list[0].effect.getTiming().easing, list[0].effect.getComputedTiming().fill].join()"
        ),
        "1,true,background-color,steps(2),backwards"
    );
}

/// A paused CSS animation is a paused `CSSAnimation`, held where its delay
/// put it; script seeking its object moves what the cascade shows.
#[test]
fn css_animation_objects_follow_play_state_and_script_control() {
    let (mut t, root) = page();
    eval(
        &root,
        "const style = document.head.appendChild(document.createElement('style'));
         style.textContent = '@keyframes grow { from { width: 100px } to { width: 200px } }' +
           ' .grow { animation: grow 1s linear -0.25s paused both }';
         globalThis.box = document.body.appendChild(document.createElement('div'));
         box.className = 'grow';
         0",
    );
    t.run_until_idle(Some(Instant::now() + Duration::from_millis(50)));
    assert_eq!(
        eval(
            &root,
            "globalThis.anim = box.getAnimations()[0];
             [anim.playState, anim.currentTime, anim.effect.getComputedTiming().fill, getComputedStyle(box).width].join()"
        ),
        "paused,0,both,125px"
    );
    assert_eq!(eval(&root, "anim.currentTime = 500; getComputedStyle(box).width"), "175px");
}

/// A style read after a seek restyles without laying out: the box read after
/// it still lays out with the new values.
#[test]
fn a_box_read_after_a_style_read_sees_a_seek() {
    let (mut t, root) = page();
    eval(
        &root,
        "globalThis.box = document.body.appendChild(document.createElement('div'));
         box.style.cssText = 'width: 100px; height: 20px';
         globalThis.anim = box.animate([{ width: '100px', opacity: 0 }, { width: '300px', opacity: 1 }], 1000);
         anim.pause(); anim.currentTime = 500; 0",
    );
    t.run_until_idle(Some(Instant::now() + Duration::from_millis(30)));
    let read = "[getComputedStyle(box).opacity, box.getBoundingClientRect().width].join()";
    assert_eq!(eval(&root, read), "0.5,200");
    assert_eq!(eval(&root, &format!("anim.currentTime = 250; {read}")), "0.25,150");
}

/// A read that asks for no style flush (`innerText` reads styles) after a
/// task in which the page came to transition: the per-task checkpoint
/// already records style changes, so the next update starts the transition
/// its class change asks for.
#[test]
fn a_change_a_read_without_a_style_flush_restyled_still_transitions() {
    let (mut t, root) = page();
    eval(
        &root,
        "globalThis.log = [];
         for (const type of ['transitionrun', 'transitionstart', 'transitionend', 'transitioncancel'])
           addEventListener(type, e => log.push(e.type + ':' + e.propertyName + ':' + e.target.id));
         const s = document.createElement('style');
         s.textContent = '.t { transition: width 20ms; width: 100px; height: 10px } .w { width: 200px }';
         document.head.appendChild(s);
         globalThis.a = document.createElement('div'); a.id = 'a'; a.className = 't'; document.body.appendChild(a);
         a.innerText; setTimeout(() => {}, 0); 0",
    );
    // The timer's task, not the update one frame after it.
    t.run_until_idle(Some(Instant::now() + Duration::from_millis(5)));
    eval(&root, "a.classList.add('w'); a.innerText; 0");
    assert_eq!(
        settle(&mut t, &root, "transitionend:width:a"),
        "transitionrun:width:a transitionstart:width:a transitionend:width:a"
    );
}

/// SMIL `<animate>` of a CSS property: computed styles read its value at the
/// document's time (discrete: the first value for the first half), a frozen
/// one keeps its last value, one that never begins (an event `begin`) and
/// one whose active duration ended without `fill="freeze"` leave the base
/// value. `keyTimes` place the values.
#[test]
fn smil_animate_of_css_properties() {
    let (mut t, root) = page();
    eval(
        &root,
        r#"document.body.innerHTML = `<svg width="100" height="100">
             <circle id="d" r="10" style="fill: red"><animate attributeName="fill" from="blue" to="green" begin="0s" dur="100s" calcMode="discrete"/></circle>
             <circle id="f" r="10" style="fill: red"><animate attributeName="fill" to="green" from="blue" dur="1ms" fill="freeze"/></circle>
             <circle id="e" r="10" style="fill: red"><animate attributeName="fill" from="blue" to="green" begin="click" dur="100s"/></circle>
             <circle id="r" r="10" style="fill: red"><animate attributeName="fill" from="blue" to="green" dur="1ms"/></circle>
             <circle id="k" r="10" style="fill: red"><animate attributeName="fill" values="blue;green;yellow" keyTimes="0;0.000001;1" dur="1000s" calcMode="discrete"/></circle>
           </svg>`; 0"#,
    );
    t.run_until_idle(Some(Instant::now() + Duration::from_millis(30)));
    assert_eq!(
        eval(
            &root,
            "['d', 'f', 'e', 'r', 'k'].map(id => getComputedStyle(document.getElementById(id)).fill).join(' ')"
        ),
        "rgb(0, 0, 255) rgb(0, 128, 0) rgb(255, 0, 0) rgb(255, 0, 0) rgb(0, 128, 0)"
    );
}

#[test]
fn a_pseudo_element_transitions_with_its_pseudo_element_in_the_events() {
    // A `::before` whose width changes transitions on its own: the events go to the element with
    // `pseudoElement` set, the element's own width does not transition, `getComputedStyle(el,
    // '::before')` reads the transition's value and its `transition-duration`, and
    // `getAnimations()` lists a CSSTransition whose effect targets the pseudo-element.
    let (mut t, root) = transition_page(
        "#a { height: 10px } #a::before { content: \"\"; display: block; width: 100px; transition: width 10s linear } #a.w::before { width: 200px }",
    );
    eval(
        &root,
        "globalThis.plog = [];
         addEventListener('transitionrun', e => plog.push(e.propertyName + ':' + e.pseudoElement));
         globalThis.a = el('a'); getComputedStyle(a, '::before').width; a.classList.add('w'); 0",
    );
    run_until(&mut t, &root, "plog.length > 0");
    assert_eq!(eval(&root, "plog.join(' ')"), "width:::before");
    assert_eq!(eval(&root, "getComputedStyle(a, '::before').transitionDuration"), "10s");
    assert_eq!(eval(&root, "getComputedStyle(a).transitionDuration"), "0s");
    let listed = eval(
        &root,
        "a.getAnimations({subtree: true}).map(x => x.constructor.name + ':' + x.effect.pseudoElement).join(' ')",
    );
    assert_eq!(listed, "CSSTransition:::before");
    let width: f64 = eval(&root, "parseFloat(getComputedStyle(a, '::before').width)").parse().unwrap();
    assert!((100.0..200.0).contains(&width), "under way: {width}");
}
