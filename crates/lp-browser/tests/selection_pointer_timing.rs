//! Selection keeps the Range object script gave it, PointerEvent init
//! members (tilt and spherical angles, coalesced events, integer coordinates
//! for mouse-type events), and User Timing (`performance.mark`/`measure`).

use lp_browser::*;
use lp_js::Realm;

fn eval(realm: &Realm, source: &str) -> String {
    let scope = realm.scope().unwrap();
    scope
        .eval(source, Some("selection-pointer-timing-test.js"), 1)
        .unwrap_or_else(|e| panic!("{}: {}", source, e.details(&scope).text()))
        .to_string()
        .unwrap()
}

fn page() -> (BrowserThread, Realm) {
    let mut t = crate::browser_thread().unwrap();
    let page = t.browser.create_page().unwrap();
    let root = t.browser.page(page).unwrap().main_frame.main_realm().unwrap().clone();
    eval(&root, "document.body.innerHTML = '<p id=a>one two</p><p id=b>three</p>'; 0");
    (t, root)
}

#[test]
fn selection_holds_the_range_object_that_was_added() {
    let (_t, root) = page();
    assert_eq!(
        eval(
            &root,
            "(() => { const sel = getSelection(), text = a.firstChild;
               const r = document.createRange(); r.setStart(text, 1); r.setEnd(text, 3);
               sel.removeAllRanges(); sel.addRange(r);
               const out = [sel.rangeCount, sel.getRangeAt(0) === r, sel.getRangeAt(0) === sel.getRangeAt(0)];
               // The selection follows the range script still holds.
               r.setEnd(b.firstChild, 2);
               out.push(sel.focusNode === b.firstChild, sel.focusOffset, sel.anchorOffset);
               // A second range and a range outside the document are ignored.
               const other = document.createRange(); other.selectNode(b);
               sel.addRange(other);
               out.push(sel.getRangeAt(0) === r);
               sel.removeAllRanges();
               const detached = document.createElement('div').appendChild(document.createTextNode('x'));
               const outside = document.createRange(); outside.setStart(detached, 0);
               sel.addRange(outside);
               out.push(sel.rangeCount);
               return out.join(); })()"
        ),
        "1,true,true,true,2,1,true,0"
    );
}

#[test]
fn selection_methods_ignore_other_trees_and_replace_the_range() {
    let (_t, root) = page();
    assert_eq!(
        eval(
            &root,
            "(() => { const sel = getSelection(), out = [];
               sel.collapse(a, 0);
               const first = sel.getRangeAt(0);
               // Nodes that are not in the document change nothing.
               const detached = document.createElement('div');
               sel.collapse(detached, 0); sel.extend(detached, 0); sel.selectAllChildren(detached);
               sel.setBaseAndExtent(detached, 0, detached, 0);
               out.push(sel.rangeCount, sel.anchorNode === a, sel.getRangeAt(0) === first);
               // collapse() sets a new range; extend() keeps the anchor and can go backwards.
               sel.collapse(b, 1);
               out.push(sel.getRangeAt(0) !== first);
               sel.extend(a, 0);
               out.push(sel.anchorNode === b, sel.focusNode === a, sel.direction, sel.getRangeAt(0).startContainer === a);
               // selectAllChildren counts children, not characters.
               sel.selectAllChildren(a.firstChild);
               out.push(sel.focusOffset);
               sel.selectAllChildren(document.body);
               out.push(sel.focusOffset === document.body.childNodes.length);
               // Only the selection's own range can be removed.
               try { sel.removeRange(document.createRange()); out.push('no throw'); } catch (e) { out.push(e.name); }
               sel.removeRange(sel.getRangeAt(0));
               out.push(sel.rangeCount);
               return out.join(); })()"
        ),
        "1,true,true,true,true,true,backward,true,0,true,NotFoundError,0"
    );
}

#[test]
fn a_selection_across_a_shadow_boundary_has_a_collapsed_range() {
    let (_t, root) = page();
    assert_eq!(
        eval(
            &root,
            "(() => { const host = document.body.appendChild(document.createElement('div'));
               const shadow = host.attachShadow({ mode: 'open' });
               shadow.innerHTML = '<span>InnerText</span>';
               const inner = shadow.firstChild.firstChild, outer = a.firstChild, sel = getSelection();
               sel.setBaseAndExtent(outer, 3, inner, 5);
               const range = sel.getRangeAt(0);
               const out = [sel.anchorNode === outer, sel.anchorOffset, sel.focusNode === inner, sel.focusOffset,
                            range.collapsed, range.startContainer === inner, range.startOffset];
               // The range spans nothing: extracting it removes nothing.
               const fragment = range.extractContents();
               out.push(fragment.childNodes.length, a.textContent, shadow.innerHTML);
               return out.join(); })()"
        ),
        "true,3,true,5,true,true,5,0,one two,<span>InnerText</span>"
    );
}

#[test]
fn pointer_event_init_members() {
    let (_t, root) = page();
    assert_eq!(
        eval(
            &root,
            "(() => { const frac = { screenX: 0.25, clientX: 1.75, clientY: -0.5 };
               const down = new PointerEvent('pointerdown', frac), click = new PointerEvent('click', frac);
               const mouse = new MouseEvent('mousemove', frac);
               const tilted = new PointerEvent('pointermove', { tiltX: 0, tiltY: 90 });
               const angled = new PointerEvent('pointermove', { altitudeAngle: Math.PI / 4, azimuthAngle: 0 });
               const plain = new PointerEvent('pointermove');
               const inner = new PointerEvent('pointermove', { clientX: 5 });
               const outer = new PointerEvent('pointermove', { coalescedEvents: [inner], predictedEvents: [inner, inner],
                                                              persistentDeviceId: 1001, twist: 7, tangentialPressure: 0.5 });
               return [down.screenX, down.clientX, down.clientY, click.screenX, click.clientX, click.clientY,
                       mouse.clientX, tilted.azimuthAngle === Math.PI / 2, tilted.altitudeAngle,
                       angled.tiltX, angled.tiltY, plain.altitudeAngle === Math.PI / 2, plain.azimuthAngle,
                       outer.getCoalescedEvents()[0] === inner, outer.getPredictedEvents().length,
                       outer.persistentDeviceId, outer.twist, outer.tangentialPressure, plain.getCoalescedEvents().length].join(); })()"
        ),
        "0.25,1.75,-0.5,0,1,-1,1,true,0,45,0,true,0,true,2,1001,7,0.5,0"
    );
}

#[test]
fn user_timing_marks_and_measures() {
    let (_t, root) = page();
    assert_eq!(
        eval(
            &root,
            "(() => { const out = [];
               const m1 = performance.mark('a', { startTime: 10, detail: { n: 1 } });
               const m2 = performance.mark('b', { startTime: 30 });
               out.push(m1 instanceof PerformanceMark, m1.entryType, m1.startTime, m1.duration, m1.detail.n, m2.detail);
               const span = performance.measure('a-b', 'a', 'b');
               out.push(span instanceof PerformanceMeasure, span.startTime, span.duration);
               const opts = performance.measure('opts', { start: 5, duration: 20, detail: 'd' });
               out.push(opts.startTime, opts.duration, opts.detail);
               out.push(performance.getEntriesByType('mark').map(e => e.name).join('/'),
                        performance.getEntriesByName('a')[0] === m1,
                        performance.getEntries().filter(e => e.entryType !== 'paint').map(e => e.name).join('/'));
               performance.clearMarks('a');
               out.push(performance.getEntriesByType('mark').length);
               performance.clearMeasures();
               out.push(performance.getEntriesByType('measure').length);
               for (const bad of [() => performance.mark(), () => performance.mark('navigationStart'),
                                  () => performance.mark('x', { startTime: -1 }), () => performance.measure('x', 'missing'),
                                  () => performance.measure('x', { start: 1 }, 'b'), () => performance.measure('x', { detail: 1 })]) {
                 try { bad(); out.push('no throw'); } catch (e) { out.push(e.name); }
               }
               const made = new PerformanceMark('made', { startTime: 2 });
               out.push(made.startTime, performance.getEntriesByName('made').length, JSON.stringify(made.toJSON().detail));
               return out.join(); })()"
        ),
        "true,mark,10,0,1,,true,10,20,5,20,d,a/b,true,opts/a/a-b/b,1,0,\
         TypeError,SyntaxError,TypeError,SyntaxError,TypeError,TypeError,2,0,null"
    );
}

#[test]
fn accessor_slots_fit_the_registry() {
    // Every attribute of every registered class gets a custom accessor slot;
    // one that does not is installed as an ordinary accessor, which the
    // inline caches cannot call directly (a silent slowdown). Raise SLOTS in
    // crates/lp-js/build.rs when this fails.
    let (_t, root) = page();
    assert_eq!(eval(&root, "typeof document.body.childNodes.length"), "number");
    let (used, overflowed) = lp_js::accessor_slot_usage();
    assert!(used > 0, "no custom accessors in use");
    assert_eq!(overflowed, 0, "{overflowed} attributes found no accessor slot ({used} in use)");
}
