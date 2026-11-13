use pyro_browser::{BrowserThread, PageId};
use pyro_js::Realm;

fn eval(realm: &Realm, source: &str) -> String {
    let scope = realm.scope().unwrap();
    scope
        .eval(source, Some("frame-focus-test.js"), 1)
        .unwrap_or_else(|e| panic!("{source}: {}", e.details(&scope).text()))
        .to_string()
        .unwrap()
}
fn setup() -> (BrowserThread, PageId, Realm) {
    let mut t = crate::browser_thread().unwrap();
    let page = t.browser.create_page().unwrap();
    let realm = t.browser.page(page).unwrap().main_frame.main_realm().unwrap().clone();
    eval(&realm, "globalThis.a=document.createElement('input');a.id='root';document.body.append(a);globalThis.f=document.createElement('iframe');f.id='owner';document.body.append(f);globalThis.c=f.contentDocument.createElement('input');c.id='child';f.contentDocument.body.append(c)");
    (t, page, realm)
}

#[test]
fn child_focus_updates_ancestor_and_parent_focus_blurs_child() {
    let (_t, _page, r) = setup();
    eval(&r, "globalThis.events=[];c.addEventListener('blur',()=>events.push('child-blur'));a.focus();c.focus()");
    assert_eq!(eval(&r, "document.activeElement.id+':'+f.contentDocument.activeElement.id"), "owner:child");
    eval(&r, "a.focus()");
    assert_eq!(
        eval(&r, "document.activeElement.id+':'+f.contentDocument.activeElement.tagName+':'+events.join(',')"),
        "root:BODY:child-blur"
    );
}

#[test]
fn window_and_owner_focus_clear_local_element_but_blur_retains_frame() {
    let (_t, _page, r) = setup();
    eval(&r, "a.focus();c.focus();window.focus()");
    assert_eq!(eval(&r, "document.activeElement.tagName+':'+f.contentDocument.activeElement.tagName"), "BODY:BODY");
    eval(&r, "c.focus();c.blur()");
    assert_eq!(eval(&r, "document.activeElement.id+':'+f.contentDocument.activeElement.tagName"), "owner:BODY");
    eval(&r, "c.focus();a.focus();f.focus()");
    assert_eq!(eval(&r, "document.activeElement.id+':'+f.contentDocument.activeElement.tagName"), "owner:BODY");
}

#[test]
fn reentrant_cross_document_focus_wins_over_outer_transition() {
    let (_t, _page, r) = setup();
    eval(&r, "globalThis.g=document.createElement('iframe');g.id='other';document.body.append(g);globalThis.b=g.contentDocument.createElement('input');b.id='newest';g.contentDocument.body.append(b);c.focus();c.addEventListener('blur',()=>b.focus(),{once:true});a.focus()");
    assert_eq!(eval(&r, "document.activeElement.id+':'+f.contentDocument.activeElement.tagName+':'+g.contentDocument.activeElement.id"), "other:BODY:newest");
}

#[test]
fn focused_snapshot_is_page_local_and_inspector_worlds_do_not_select_it() {
    let (mut t, page, r) = setup();
    let root = t.browser.page(page).unwrap().main_frame.id.clone();
    assert_eq!(t.browser.focused_frame(page).unwrap().id, root);
    let other = t.browser.create_page().unwrap();
    eval(&r, "c.focus()");
    let child = t.browser.focused_frame(page).unwrap().id;
    assert_ne!(child, root);
    let isolated = t.browser.isolated_world(page, "inspector").unwrap();
    eval(&isolated, "document.body.tagName");
    assert_eq!(t.browser.focused_frame(page).unwrap().id, child);
    assert_eq!(t.browser.focused_frame(other).unwrap().id, t.browser.page(other).unwrap().main_frame.id);
    eval(&r, "c.blur()");
    assert_eq!(t.browser.focused_frame(page).unwrap().id, child);
    eval(&r, "window.focus()");
    assert_eq!(t.browser.focused_frame(page).unwrap().id, root);
    assert!(t.browser.close_page(page));
    assert!(t.browser.focused_frame(page).is_none());
}

#[test]
fn removal_and_navigation_invalidate_targets_without_restoring_parent_input() {
    use std::time::{Duration, Instant};
    let (mut t, page, r) = setup();
    eval(&r, "a.focus();c.focus()");
    let old = t.browser.focused_frame(page).unwrap();
    eval(&r, "f.srcdoc='<input id=new>'");
    t.run_until_idle(Some(Instant::now() + Duration::from_secs(3)));
    let current = t.browser.focused_frame(page).unwrap();
    assert_eq!(current.id, old.id);
    assert_ne!(current.loader_id, old.loader_id);
    assert!(old.page_dom.unwrap().is_disposed());
    assert_eq!(eval(current.main_realm().unwrap(), "document.activeElement.tagName"), "BODY");
    eval(&r, "f.contentDocument.querySelector('input').focus();f.remove()");
    assert_eq!(t.browser.focused_frame(page).unwrap().id, t.browser.page(page).unwrap().main_frame.id);
    assert_eq!(eval(&r, "document.activeElement.tagName+':'+a.value"), "BODY:");
    t.browser.checkpoint();
    assert!(current.page_dom.unwrap().is_disposed());
}

#[test]
fn nested_chain_and_sandboxed_window_focus_use_live_frame_identity() {
    use std::time::{Duration, Instant};
    let (mut t, page, r) = setup();
    eval(&r, "globalThis.g=f.contentDocument.createElement('iframe');g.id='nested';f.contentDocument.body.append(g);globalThis.n=g.contentDocument.createElement('input');g.contentDocument.body.append(n);n.focus()");
    assert_eq!(eval(&r, "document.activeElement.id+':'+f.contentDocument.activeElement.id+':'+g.contentDocument.activeElement.tagName"), "owner:nested:INPUT");
    let nested = t.browser.focused_frame(page).unwrap().id;
    eval(&r, "g.remove()");
    assert_eq!(t.browser.focused_frame(page).unwrap().id, t.browser.page(page).unwrap().main_frame.id);
    assert_eq!(eval(&r, "document.activeElement.tagName+':'+f.contentDocument.activeElement.tagName"), "BODY:BODY");
    let parent = t.browser.frames(page).into_iter().find(|f| f.parent.is_some()).unwrap();
    assert_ne!(nested, parent.id);
    eval(&r, "f.setAttribute('sandbox','');f.srcdoc='<input id=opaque>'");
    t.run_until_idle(Some(Instant::now() + Duration::from_secs(3)));
    let child = t.browser.frame(page, &parent.id).unwrap();
    assert_eq!(child.origin, "null");
    eval(child.main_realm().unwrap(), "document.querySelector('input').focus()");
    assert_eq!(t.browser.focused_frame(page).unwrap().id, child.id);
    eval(&r, "a.focus();f.contentWindow.focus()");
    assert_eq!(t.browser.focused_frame(page).unwrap().id, child.id);
    assert_eq!(eval(child.main_realm().unwrap(), "document.activeElement.tagName"), "BODY");
}

#[test]
fn removal_during_blur_cancels_pending_child_focus() {
    let (t, page, r) = setup();
    eval(&r, "a.focus();a.addEventListener('blur',()=>f.remove(),{once:true});c.focus()");
    assert_eq!(t.browser.focused_frame(page).unwrap().id, t.browser.page(page).unwrap().main_frame.id);
    assert_eq!(eval(&r, "document.activeElement.tagName"), "BODY");
}

#[test]
fn stale_child_request_during_blur_cannot_cancel_live_transition() {
    let (t, page, r) = setup();
    eval(&r, "globalThis.g=document.createElement('iframe');g.id='live';document.body.append(g);globalThis.b=g.contentDocument.createElement('input');b.id='new';g.contentDocument.body.append(b);c.focus();a.focus();f.remove();a.addEventListener('blur',()=>c.focus(),{once:true});b.focus()");
    assert_eq!(eval(&r, "document.activeElement.id+':'+g.contentDocument.activeElement.id"), "live:new");
    assert_eq!(
        t.browser.focused_frame(page).unwrap().id,
        t.browser.frames(page).into_iter().find(|f| f.parent.is_some()).unwrap().id
    );
}
