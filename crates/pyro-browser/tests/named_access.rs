//! Named access on the Window object: a named properties object behind the
//! global's own properties.
use pyro_browser::*;
use pyro_js::Realm;
use std::time::{Duration, Instant};

fn eval(realm: &Realm, source: &str) -> String {
    let scope = realm.scope().unwrap();
    scope
        .eval(source, Some("named-access-test.js"), 1)
        .unwrap_or_else(|e| panic!("{}: {}", source, e.details(&scope).text()))
        .to_string()
        .unwrap()
}

fn page_realm(t: &mut BrowserThread) -> (PageId, Realm) {
    let page = t.browser.create_page().unwrap();
    let realm = t.browser.page(page).unwrap().main_frame.main_realm().unwrap().clone();
    eval(
        &realm,
        r#"document.body.innerHTML = `
            <div id=foo></div>
            <form name=f></form><img name=i><embed name=em><object name=ob></object>
            <input name=notnamed><a name=alsonot></a>
            <div id=dup></div><form name=dup></form>
            <div id=toString></div><div id=document></div><div id=assigned></div>
            <div id=host></div>`;
           host.attachShadow({mode:'open'}).innerHTML = '<div id=inshadow></div>';"#,
    );
    (page, realm)
}

#[test]
fn elements_resolve_by_id_and_name_behind_own_properties() {
    let mut t = crate::browser_thread().unwrap();
    let (_, r) = page_realm(&mut t);
    // Bare, through `window`, and through `in`, by id and by name.
    assert_eq!(eval(&r, "foo === document.getElementById('foo') && window.foo === foo && 'foo' in window"), "true");
    assert_eq!(eval(&r, "[f.tagName, i.tagName, em.tagName, ob.tagName].join()"), "FORM,IMG,EMBED,OBJECT");
    // Other elements' names do not count; shadow trees are not searched.
    assert_eq!(
        eval(&r, "[typeof notnamed, typeof alsonot, typeof inshadow, typeof window[''], 'nope' in window].join()"),
        "undefined,undefined,undefined,undefined,false"
    );
    assert!(r.scope().unwrap().eval("missing", None, 1).is_err(), "a name no element has still throws");
    // Several elements: a live collection in tree order.
    assert_eq!(eval(&r, "dup instanceof HTMLCollection && dup.length === 2 && dup[1].tagName === 'FORM'"), "true");
    let live =
        "const c = dup; document.querySelector('form[name=dup]').remove(); c.length === 1 && dup.tagName === 'DIV'";
    assert_eq!(eval(&r, live), "true");
    // Own properties and Object.prototype win.
    assert_eq!(eval(&r, "document.nodeType === 9 && toString === Object.prototype.toString"), "true");
    // Not own, not enumerable.
    assert_eq!(
        eval(&r, "!window.hasOwnProperty('foo') && !Object.keys(window).includes('foo') && (()=>{for(const k in window) if(k==='foo') return false; return true})()"),
        "true"
    );
    // window -> Window.prototype -> WindowProperties -> EventTarget.prototype, as in Chrome (f47805459).
    assert_eq!(
        eval(&r, "const p = Object.getPrototypeOf(window), named = Object.getPrototypeOf(p); [p === Window.prototype, String(named), Object.getPrototypeOf(named) === EventTarget.prototype].join()"),
        "true,[object WindowProperties],true"
    );
}

#[test]
fn names_are_live_and_assignment_and_declarations_shadow_them() {
    let mut t = crate::browser_thread().unwrap();
    let (_, r) = page_realm(&mut t);
    // A site that resolved a name sees it change, and sees it go.
    eval(&r, "globalThis.read = () => typeof late === 'undefined' ? 'gone' : late.tagName");
    assert_eq!(eval(&r, "read()"), "gone");
    eval(&r, "const el = document.createElement('p'); el.id = 'late'; document.body.append(el)");
    assert_eq!(eval(&r, "read()"), "P");
    eval(&r, "document.getElementById('late').id = 'renamed'");
    assert_eq!(eval(&r, "read() + ':' + renamed.tagName"), "gone:P");
    // Assignment creates an own property; deleting it brings the element back.
    assert_eq!(
        eval(&r, "assigned = 1; const own = window.hasOwnProperty('assigned') && assigned === 1; delete window.assigned; own && assigned.tagName === 'DIV'"),
        "true"
    );
    assert_eq!(eval(&r, "'use strict'; (function(){'use strict'; foo = 2; return foo})()"), "2");
    // `var` and function declarations are own properties: they shadow it.
    assert_eq!(eval(&r, "var f; typeof f"), "undefined");
    assert_eq!(eval(&r, "function i() {} typeof i"), "function");
    // `let` lives in the lexical scope, in front of the global.
    assert_eq!(eval(&r, "let em = 3; em + ':' + window.em.tagName"), "3:EMBED");
}

#[test]
fn child_windows_answer_for_iframe_names_and_realms_are_kept_apart() {
    let mut t = crate::browser_thread().unwrap();
    let (page, r) = page_realm(&mut t);
    eval(
        &r,
        "globalThis.fr = document.createElement('iframe'); fr.name = 'child'; fr.id = 'childid'; \
         fr.srcdoc = '<p id=inside></p>'; document.body.append(fr)",
    );
    t.run_until_idle(Some(Instant::now() + Duration::from_secs(3)));
    assert_eq!(eval(&r, "child === fr.contentWindow && childid === fr"), "true");
    // The child's names live in its own window, and wrap in its realm.
    assert_eq!(eval(&r, "typeof inside === 'undefined' && child.inside.ownerDocument === fr.contentDocument"), "true");
    assert_eq!(eval(&r, "child.inside instanceof child.HTMLElement"), "true");
    // Isolated worlds see the same names through their own wrappers, and
    // their own globals shadow them there only.
    let world = t.browser.isolated_world(page, "utility").unwrap();
    assert_eq!(eval(&world, "foo instanceof HTMLDivElement && foo === document.getElementById('foo')"), "true");
    eval(&world, "globalThis.foo = 'mine'");
    assert_eq!(eval(&world, "foo"), "mine");
    assert_eq!(eval(&r, "foo.tagName"), "DIV");
}

#[test]
fn a_miss_does_not_walk_the_tree_after_each_mutation() {
    let mut t = crate::browser_thread().unwrap();
    let (_, r) = page_realm(&mut t);
    // Quadratic if a miss rebuilt its index: well past the timeout on a debug build.
    assert_eq!(
        eval(
            &r,
            "let misses = 0; for (let i = 0; i < 2000; i++) { document.body.append(document.createElement('p')); }
             for (let i = 0; i < 2000; i++) { document.body.append(document.createElement('p')); if (typeof process === 'undefined') misses++; }
             misses"
        ),
        "2000"
    );
}
