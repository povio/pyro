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
