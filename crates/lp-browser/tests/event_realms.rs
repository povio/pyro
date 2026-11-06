//! Events across the windows of a frame tree and through the browser: the
//! realm a listener callback runs, exposes `window.event` and reports its
//! exception in (the callback's own, DOM "inner invoke"), `Performance` as an
//! EventTarget sharing its clock with `Event.timeStamp`, `beforeunload` on
//! navigation, CSS animation and transition events, and the viewport's
//! scrolling element.

use lp_browser::*;
use lp_js::Realm;
use std::time::{Duration, Instant};

fn eval(realm: &Realm, source: &str) -> String {
    let scope = realm.scope().unwrap();
    scope
        .eval(source, Some("event-realms-test.js"), 1)
        .unwrap_or_else(|e| panic!("{}: {}", source, e.details(&scope).text()))
        .to_string()
        .unwrap()
}
fn page_with_frames() -> (BrowserThread, Realm) {
    let mut t = crate::browser_thread().unwrap();
    let page = t.browser.create_page().unwrap();
    let root = t.browser.page(page).unwrap().main_frame.main_realm().unwrap().clone();
    eval(
        &root,
        "globalThis.frame = () => { const f = document.createElement('iframe'); document.body.appendChild(f); return f; };
         globalThis.a = frame(); globalThis.b = frame(); globalThis.A = a.contentWindow; globalThis.B = b.contentWindow; 0",
    );
    (t, root)
}
/// Runs the page until `done` holds (events fired by timers), at most 5 s.
fn run_until(thread: &mut BrowserThread, root: &Realm, done: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        thread.run_until_idle(Some(Instant::now() + Duration::from_millis(50)));
        if eval(root, done) == "true" {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn window_event_is_the_callback_realms() {
    let (_t, root) = page_with_frames();
    assert_eq!(
        eval(
            &root,
            "(() => { const out = [];
               // A target of frame A, a listener of this window.
               const target = new A.EventTarget(), hi = new Event('hi');
               target.addEventListener('hi', e => out.push(A.event === undefined, window.event === e));
               target.dispatchEvent(hi);
               // A listener function of frame A on this window's document.
               A.eval('globalThis.listener = function (e) { parent.frameState = [e === parent.yo, window.event === e, parent.event === undefined].join(); }');
               globalThis.yo = new Event('yo');
               document.addEventListener('yo', A.listener);
               document.dispatchEvent(yo);
               out.push(frameState);
               // An event handler function of frame A on this window.
               window.onresize = new A.Function('parent.handlerState = [parent.event === undefined, window.event.type].join()');
               window.dispatchEvent(new Event('resize'));
               out.push(handlerState, window.event === undefined, A.event === undefined);
               return out.join(); })()"
        ),
        "true,true,true,true,true,true,resize,true,true"
    );
}

#[test]
fn a_listeners_exception_is_reported_in_the_callbacks_realm() {
    let (_t, root) = page_with_frames();
    assert_eq!(
        eval(
            &root,
            "(() => { const errors = [];
               const kind = (err) => { for (const [name, w] of [['root', window], ['A', A], ['B', B]]) {
                   if (err.constructor === w.TypeError) return name + '.TypeError';
                   if (err.constructor === w.Error) return name + '.Error'; } return 'other'; };
               for (const [name, w] of [['root', window], ['A', A], ['B', B]])
                 w.addEventListener('error', ev => { errors.push(name + ':' + kind(ev.error)); ev.preventDefault(); });
               const fire = (target, listener) => { target.addEventListener('p', listener); target.dispatchEvent(new Event('p')); };
               // A function of A on a target of B.
               A.eval('globalThis.thrower = () => { throw new Error(1); }; globalThis.object = () => ({})');
               fire(new B.EventTarget(), A.thrower);
               // A callback object of A whose handleEvent is a function of B.
               B.eval('globalThis.handle = () => { throw new Error(2); }');
               fire(new EventTarget(), Object.assign(A.object(), { handleEvent: B.handle }));
               // No handleEvent: the TypeError of the object's realm.
               fire(new EventTarget(), A.object());
               // A revoked Proxy of this window as handleEvent of A's object.
               const own = Proxy.revocable(() => {}, {}); own.revoke();
               fire(new EventTarget(), Object.assign(A.object(), { handleEvent: own.proxy }));
               // A callable revoked Proxy of B as the listener.
               const other = B.Proxy.revocable(() => {}, {}); other.revoke();
               fire(new EventTarget(), other.proxy);
               return errors.join(); })()"
        ),
        "A:A.Error,A:B.Error,A:A.TypeError,A:A.TypeError,B:B.TypeError"
    );
}

#[test]
fn performance_is_an_event_target_and_event_time_stamps_use_its_clock() {
    let (_t, root) = page_with_frames();
    assert_eq!(
        eval(
            &root,
            "(() => { let current; const ev = new Event('t', { cancelable: true });
               performance.addEventListener('t', e => { current = window.event; e.preventDefault(); });
               const canceled = !performance.dispatchEvent(ev);
               const before = performance.now(), event = new MouseEvent('x'), after = performance.now();
               const steps = performance.now() * 10;
               return [performance instanceof EventTarget, performance === performance, current === ev, canceled,
                       window.event === undefined, before <= event.timeStamp && event.timeStamp <= after,
                       Math.abs(steps - Math.round(steps)) < 1e-6, window.constructor === Window].join(); })()"
        ),
        "true,true,true,true,true,true,true,true"
    );
}

#[test]
fn navigating_a_frame_fires_beforeunload_at_its_window() {
    let (mut t, root) = page_with_frames();
    eval(
        &root,
        "A.eval(\"onbeforeunload = (e) => ({ toString() { parent.unloading = [window.event === e, e.type, e.cancelable].join(); return 'stay?'; } })\");
         A.location.href = 'about:blank'; 0",
    );
    run_until(&mut t, &root, "typeof unloading === 'string'");
    assert_eq!(eval(&root, "String(globalThis.unloading)"), "true,beforeunload,true");
}

#[test]
fn css_animations_and_transitions_fire_their_events() {
    let (mut t, root) = page_with_frames();
    eval(
        &root,
        "const style = document.createElement('style');
         style.textContent = '@keyframes spin {} .t { width: 100px; transition: width 1ms }';
         document.head.appendChild(style);
         globalThis.log = [];
         for (const type of ['animationstart', 'animationiteration', 'animationend', 'animationcancel', 'transitionrun', 'transitionstart', 'transitionend'])
           addEventListener(type, e => log.push([e.type, e.animationName || e.propertyName, e.elapsedTime, e.isTrusted, e.constructor.name, e.target === el].join(':')));
         globalThis.el = document.body.appendChild(document.createElement('div'));
         el.className = 't';
         el.addEventListener('webkitAnimationEnd', e => log.push('legacy:' + e.type));
         getComputedStyle(el).width;
         el.style.animation = 'spin 2ms 2';
         el.style.width = '200px'; 0",
    );
    run_until(&mut t, &root, "log.some(entry => entry.startsWith('animationend'))");
    assert_eq!(
        eval(&root, "log.join(' ')"),
        "animationstart:spin:0:true:AnimationEvent:true transitionrun:width:0:true:TransitionEvent:true \
         transitionstart:width:0:true:TransitionEvent:true transitionend:width:0.001:true:TransitionEvent:true \
         animationiteration:spin:0.002:true:AnimationEvent:true legacy:webkitAnimationEnd \
         animationend:spin:0.004:true:AnimationEvent:true"
    );
    // A finished animation does not start again; one that leaves the style
    // before its end is canceled, and an infinite one only starts.
    eval(&root, "log.length = 0; el.style.animation = 'spin 1s infinite'; 0");
    run_until(&mut t, &root, "log.length > 0");
    eval(&root, "el.style.animation = ''; 0");
    run_until(&mut t, &root, "log.length > 1");
    assert_eq!(
        eval(&root, "log.join(' ')"),
        "animationstart:spin:0:true:AnimationEvent:true animationcancel:spin:0:true:AnimationEvent:true"
    );
}

#[test]
fn a_first_style_read_fires_nothing_and_a_later_read_after_a_change_fires_its_transition() {
    // A first read only records the before-change style (the sheets' timing
    // rules are not needed for it); a second read after a change reads them
    // and starts the transition. An element first read never transitions.
    let (mut t, root) = page_with_frames();
    eval(
        &root,
        "globalThis.log = [];
         for (const type of ['animationstart', 'transitionrun'])
           addEventListener(type, e => log.push(e.type + ':' + (e.animationName || e.propertyName) + ':' + e.target.id));
         const style = document.head.appendChild(document.createElement('style'));
         style.textContent = '@keyframes grow {} .t { width: 100px; transition: width 1s } .w { width: 200px } .a { animation: grow 1s infinite }'; 0",
    );
    t.run_until_idle(Some(Instant::now() + Duration::from_millis(50)));
    eval(
        &root,
        "for (const id of ['el', 'first', 'animated']) {
           const e = document.body.appendChild(document.createElement('div'));
           e.id = id;
         }
         el.className = 't';
         getComputedStyle(el).width;
         el.className = 't w';
         getComputedStyle(el).width;
         first.className = 't w';
         getComputedStyle(first).width;
         animated.className = 'a';
         getComputedStyle(animated).animationName; 0",
    );
    run_until(&mut t, &root, "log.length >= 2");
    t.run_until_idle(Some(Instant::now() + Duration::from_millis(50)));
    let mut log: Vec<String> = eval(&root, "log.join(' ')").split(' ').map(str::to_owned).collect();
    log.sort();
    assert_eq!(log, ["animationstart:grow:animated", "transitionrun:width:el"]);
}

#[test]
fn a_style_text_edit_reaches_the_timing_rules_within_the_same_task() {
    // The sheets' timing rules follow a `<style>`'s text as soon as it is
    // edited, before the task's style checkpoint: `@keyframes` added there
    // start the animation, and a transition added there runs on the next read.
    let (mut t, root) = page_with_frames();
    eval(
        &root,
        "globalThis.log = [];
         for (const type of ['animationstart', 'transitionrun'])
           addEventListener(type, e => log.push(e.type + ':' + (e.animationName || e.propertyName) + ':' + e.target.id));
         globalThis.style = document.head.appendChild(document.createElement('style'));
         style.textContent = '.t { width: 100px } .w { width: 200px } .a { animation: grow 1s infinite }';
         for (const id of ['el', 'animated']) {
           const e = document.body.appendChild(document.createElement('div'));
           e.id = id;
         }
         el.className = 't';
         animated.className = 'a';
         getComputedStyle(el).width;
         getComputedStyle(animated).animationName; 0",
    );
    t.run_until_idle(Some(Instant::now() + Duration::from_millis(50)));
    assert_eq!(eval(&root, "log.join(' ')"), "");
    eval(
        &root,
        "style.firstChild.data = '@keyframes grow {} .t { width: 100px; transition: width 1s } .w { width: 200px } .a { animation: grow 1s infinite }';
         getComputedStyle(animated).animationName;
         el.className = 't w';
         getComputedStyle(el).width; 0",
    );
    run_until(&mut t, &root, "log.length >= 2");
    let mut log: Vec<String> = eval(&root, "log.join(' ')").split(' ').map(str::to_owned).collect();
    log.sort();
    assert_eq!(log, ["animationstart:grow:animated", "transitionrun:width:el"]);
}

#[test]
fn the_scrolling_elements_offsets_are_the_viewports() {
    let (mut t, root) = page_with_frames();
    assert_eq!(
        eval(
            &root,
            "(() => { document.body.style.height = '5000px'; globalThis.scrolls = [];
               addEventListener('scroll', e => scrolls.push(e.target === document));
               const out = [document.scrollingElement === document.documentElement];
               document.scrollingElement.scrollTop = 120;
               out.push(window.scrollY, document.documentElement.scrollTop);
               window.scrollTo(0, 40);
               out.push(document.documentElement.scrollTop);
               document.documentElement.scrollBy(0, 10);
               out.push(window.scrollY);
               // A text input scrolls its value.
               const input = document.body.appendChild(document.createElement('input'));
               input.style.width = '100px'; input.value = 'q'.repeat(80);
               input.scrollLeft = 10;
               out.push(input.scrollLeft, input.scrollWidth > input.clientWidth);
               return out.join(); })()"
        ),
        "true,120,120,40,50,10,true"
    );
    run_until(&mut t, &root, "scrolls.length > 0");
    assert_eq!(eval(&root, "scrolls.join()"), "true");
}
