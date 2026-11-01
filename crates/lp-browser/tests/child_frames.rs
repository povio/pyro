use lp_browser::*;
use lp_js::Realm;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

fn eval(realm: &Realm, source: &str) -> String {
    let scope = realm.scope().unwrap();
    scope
        .eval(source, Some("frames-test.js"), 1)
        .unwrap_or_else(|e| panic!("{}: {}", source, e.details(&scope).text()))
        .to_string()
        .unwrap()
}
fn main_realm(thread: &BrowserThread, page: PageId) -> Realm {
    thread.browser.page(page).unwrap().main_frame.main_realm().unwrap().clone()
}
fn settle(thread: &mut BrowserThread) {
    thread.run_until_idle(Some(Instant::now() + Duration::from_secs(3)));
}
#[derive(Default)]
struct Events(RefCell<Vec<PageEvent>>);
impl PageObserver for Events {
    fn on_page_event(&self, _: PageId, event: &PageEvent) {
        self.0.borrow_mut().push(event.clone());
    }
}

#[test]
fn initial_blank_is_synchronous_and_srcdoc_owns_its_document_and_worlds() {
    let mut t = crate::browser_thread().unwrap();
    let page = t.browser.create_page().unwrap();
    let root = main_realm(&t, page);
    assert_eq!(eval(&root,"globalThis.f=document.createElement('iframe'); document.body.appendChild(f); f.contentWindow !== null && f.contentDocument.URL === 'about:blank' && f.contentDocument === f.contentWindow.document && f.contentWindow.parent === window && f.contentWindow.top === window && f.contentWindow.frameElement === f && length === 1"),"true");
    assert_eq!(t.browser.frames(page).len(), 2);
    eval(&root, "f.srcdoc='<p id=inside>child</p><script>globalThis.ran=42</script>'");
    settle(&mut t);
    let child = t.browser.frames(page).into_iter().find(|f| f.parent.is_some()).unwrap();
    assert_eq!(child.url, "about:srcdoc");
    assert!(child.loaded && child.dom_content_loaded);
    let world = t.browser.isolated_world_in_frame(page, &child.id, "utility").unwrap();
    assert_eq!(eval(&world, "document.querySelector('#inside').textContent + ':' + typeof ran"), "child:undefined");
    assert_eq!(
        eval(&root, "f.contentDocument.querySelector('#inside').textContent + ':' + f.contentWindow.ran"),
        "child:42"
    );
    eval(&world, "document.querySelector('#inside').textContent='shared'");
    assert_eq!(eval(&root, "f.contentDocument.querySelector('#inside').textContent"), "shared");
}

#[test]
fn sandboxed_srcdoc_keeps_inspector_realms_but_blocks_author_script_and_parent_access() {
    let mut t = crate::browser_thread().unwrap();
    let page = t.browser.create_page().unwrap();
    let root = main_realm(&t, page);
    eval(&root,"globalThis.f=document.createElement('iframe'); f.setAttribute('sandbox','allow-downloads'); f.srcdoc='<p>preview</p><script>globalThis.authorRan=true</script>'; document.body.appendChild(f)");
    settle(&mut t);
    let child = t.browser.frames(page).into_iter().find(|f| f.parent.is_some()).unwrap();
    assert_eq!(child.origin, "null");
    let realm = t.browser.frame(page, &child.id).unwrap().main_realm().unwrap().clone();
    assert_eq!(
        eval(&realm, "document.body.textContent.includes('preview') && typeof authorRan === 'undefined' && 6*7===42"),
        "true"
    );
    assert_eq!(eval(&root,"f.contentDocument===null && f.contentWindow!==null && f.contentWindow===f.contentWindow.self && (()=>{try{f.contentWindow.document;return false}catch(e){return e.name==='SecurityError'}})()"),"true");
    assert_eq!(eval(&realm,"frameElement===null && (()=>{try{parent.document;return false}catch(e){return e.name==='SecurityError'}})()"),"true");
}

#[test]
fn nested_removal_destroys_only_descendants_in_depth_first_order() {
    let mut t = crate::browser_thread().unwrap();
    let events = Rc::new(Events::default());
    t.browser.set_observer(Some(events.clone()));
    let page = t.browser.create_page().unwrap();
    let root = main_realm(&t, page);
    eval(&root,"globalThis.a=document.createElement('iframe');globalThis.b=document.createElement('iframe'); document.body.appendChild(a);document.body.appendChild(b);let c=a.contentDocument.createElement('iframe');a.contentDocument.body.appendChild(c)");
    settle(&mut t);
    assert_eq!(t.browser.frames(page).len(), 4);
    let before = t.browser.frames(page);
    let main = t.browser.page(page).unwrap().main_frame.id.clone();
    let parent = before.iter().find(|f| f.parent.as_ref() == Some(&main)).unwrap().id.clone();
    let nested = before.iter().find(|f| f.parent.as_ref() == Some(&parent)).unwrap().id.clone();
    let child_realm = t.browser.frame(page, &nested).unwrap().main_realm().unwrap().clone();
    events.0.borrow_mut().clear();
    eval(&root, "a.remove()");
    t.browser.checkpoint();
    assert_eq!(t.browser.frames(page).len(), 2);
    assert!(child_realm.is_disposed());
    assert_eq!(eval(&root, "b.contentWindow.parent===window && length===1"), "true");
    let e = events.0.borrow();
    let detached: Vec<_> = e
        .iter()
        .filter_map(|e| {
            if let PageEvent::FrameChildFrameDetached { frame_id, .. } = e {
                Some(frame_id.clone())
            } else {
                None
            }
        })
        .collect();
    assert_eq!(detached, vec![nested, parent]);
    for (i, event) in e.iter().enumerate() {
        if let PageEvent::FrameChildFrameDetached { frame_id, .. } = event {
            assert!(e[..i].iter().any(|e| matches!(e,PageEvent::RealmDestroyed{frame_id:f,..}if f==frame_id)));
        }
    }
}

#[test]
fn parent_load_waits_for_http_child_and_child_has_its_own_request_identity() {
    let mut t = crate::browser_thread().unwrap();
    let loader = Rc::new(StaticLoader::default());
    loader.insert("https://example.test/main", "text/html", "<iframe src='/child'></iframe>", Duration::ZERO);
    loader.insert("https://example.test/child", "text/html", "<p>child</p>", Duration::from_millis(20));
    t.browser.set_loader(Some(loader));
    let events = Rc::new(Events::default());
    t.browser.set_observer(Some(events.clone()));
    let page = t.browser.create_page().unwrap();
    events.0.borrow_mut().clear();
    t.browser.navigate(page, NavigateOpts::address_bar("https://example.test/main")).unwrap();
    settle(&mut t);
    let frames = t.browser.frames(page);
    assert_eq!(frames.len(), 2);
    let root = frames.iter().find(|f| f.parent.is_none()).unwrap();
    let child = frames.iter().find(|f| f.parent.is_some()).unwrap();
    assert_eq!(child.url, "https://example.test/child");
    assert_ne!(root.loader_id, child.loader_id);
    assert!(root.loaded && child.loaded);
    let e = events.0.borrow();
    let loaded: Vec<_> = e
        .iter()
        .filter_map(|e| if let PageEvent::FrameLoaded { frame_id, .. } = e { Some(frame_id) } else { None })
        .collect();
    assert_eq!(loaded, vec![&child.id, &root.id]);
}

#[test]
fn child_location_navigation_keeps_parent_and_sibling_realms() {
    let mut t = crate::browser_thread().unwrap();
    let loader = Rc::new(StaticLoader::default());
    loader.insert("https://example.test/next", "text/html", "<p>next</p>", Duration::ZERO);
    t.browser.set_loader(Some(loader));
    let page = t.browser.create_page().unwrap();
    let root = main_realm(&t, page);
    eval(&root,"globalThis.a=document.createElement('iframe');globalThis.b=document.createElement('iframe');document.body.appendChild(a);document.body.appendChild(b)");
    settle(&mut t);
    let frames = t.browser.frames(page);
    let child = &frames[1];
    let sibling = t.browser.frame(page, &frames[2].id).unwrap().main_realm().unwrap().clone();
    let old = t.browser.frame(page, &child.id).unwrap().main_realm().unwrap().clone();
    eval(&old, "location.href='https://example.test/next'");
    settle(&mut t);
    assert_eq!(t.browser.frame(page, &child.id).unwrap().url, "https://example.test/next");
    assert!(old.is_disposed());
    assert!(!root.is_disposed() && !sibling.is_disposed());
    assert_eq!(t.browser.page(page).unwrap().main_frame.url, "about:blank");
}

#[test]
fn removal_inside_child_script_finishes_the_callback_then_releases_its_realms() {
    let mut t = crate::browser_thread().unwrap();
    let page = t.browser.create_page().unwrap();
    let root = main_realm(&t, page);
    eval(&root, "globalThis.f=document.createElement('iframe');f.id='owner';document.body.appendChild(f)");
    settle(&mut t);
    let child = t.browser.frames(page).into_iter().find(|f| f.parent.is_some()).unwrap();
    let realm = t.browser.frame(page, &child.id).unwrap().main_realm().unwrap().clone();
    assert_eq!(eval(&realm, "parent.document.getElementById('owner').remove(); 6*7"), "42");
    t.browser.checkpoint();
    assert!(realm.is_disposed());
    assert_eq!(t.browser.frames(page).len(), 1);
    assert_eq!(eval(&root, "length===0 && typeof frames[0]==='undefined'"), "true");
}

#[test]
fn sandbox_changes_apply_on_navigation_and_descendants_cannot_enable_scripts() {
    let mut t = crate::browser_thread().unwrap();
    let page = t.browser.create_page().unwrap();
    let root = main_realm(&t, page);
    eval(&root, "globalThis.f=document.createElement('iframe');f.srcdoc='<p>same</p>';document.body.appendChild(f)");
    settle(&mut t);
    eval(&root,"f.setAttribute('sandbox','allow-same-origin'); f.srcdoc='<iframe sandbox=\"allow-same-origin allow-scripts\" srcdoc=\"<script>globalThis.ran=true</script>\"></iframe>'");
    settle(&mut t);
    let frames = t.browser.frames(page);
    assert_eq!(frames.len(), 3);
    for f in frames.iter().filter(|f| f.parent.is_some()) {
        let realm = t.browser.frame(page, &f.id).unwrap().main_realm().unwrap().clone();
        assert_eq!(eval(&realm, "typeof ran"), "undefined");
    }
}

#[test]
fn frame_indices_and_names_follow_removal_and_queued_navigation_is_stale_safe() {
    let mut t = crate::browser_thread().unwrap();
    let page = t.browser.create_page().unwrap();
    let root = main_realm(&t, page);
    eval(&root,"globalThis.a=document.createElement('iframe');globalThis.b=document.createElement('iframe');a.srcdoc='<p>unused</p>';document.body.appendChild(a);document.body.appendChild(b);b.name='second';a.remove()");
    settle(&mut t);
    assert_eq!(t.browser.frames(page).len(), 2);
    assert_eq!(t.browser.frames(page)[1].name, "second");
    assert_eq!(eval(&root, "frames[0]===b.contentWindow && typeof frames[1]==='undefined'"), "true");
    t.browser.navigate(page, NavigateOpts::address_bar("about:blank")).unwrap();
    settle(&mut t);
    assert_eq!(t.browser.frames(page).len(), 1);
}

#[test]
fn detached_pending_navigation_is_cancelled_and_late_completion_cannot_resurrect_it() {
    use std::cell::Cell;
    #[derive(Default)]
    struct Delayed {
        pending: RefCell<Vec<FetchDone>>,
        cancelled: Rc<Cell<usize>>,
    }
    impl ResourceLoader for Delayed {
        fn fetch(&self, _: &mut Browser, _: FetchRequest, done: FetchDone) {
            self.pending.borrow_mut().push(done);
        }
        fn fetch_cancellable(&self, b: &mut Browser, r: FetchRequest, done: FetchDone) -> Option<ResourceCancellation> {
            self.fetch(b, r, done);
            let count = self.cancelled.clone();
            Some(ResourceCancellation::new(move || count.set(count.get() + 1)))
        }
    }
    let loader = Rc::new(Delayed::default());
    let mut t = crate::browser_thread().unwrap();
    t.browser.set_loader(Some(loader.clone()));
    let page = t.browser.create_page().unwrap();
    let root = main_realm(&t, page);
    eval(
        &root,
        "globalThis.f=document.createElement('iframe');f.src='https://example.test/slow';document.body.appendChild(f)",
    );
    assert_eq!(
        t.run_until(Some(Instant::now() + Duration::from_secs(2)), |_| !loader.pending.borrow().is_empty()),
        lp_runtime::RunOutcome::Done
    );
    eval(&root, "f.remove()");
    t.browser.checkpoint();
    assert_eq!(loader.cancelled.get(), 1);
    let done = loader.pending.borrow_mut().pop().unwrap();
    done(
        &mut t.browser,
        Ok(Resource {
            url: "https://example.test/slow".into(),
            status: 200,
            mime: "text/html".into(),
            charset: None,
            body: b"<iframe></iframe><script>globalThis.late=true</script>".to_vec(),
            content_disposition: None,
        }),
    );
    settle(&mut t);
    assert_eq!(t.browser.frames(page).len(), 1);
    assert_eq!(eval(&root, "typeof late"), "undefined");
}

#[test]
fn repeated_frame_removal_releases_native_realms_without_retaining_the_page() {
    let mut t = crate::browser_thread().unwrap();
    let page = t.browser.create_page().unwrap();
    let root = main_realm(&t, page);
    let mut weak = Vec::new();
    for _ in 0..20 {
        eval(&root, "globalThis.f=document.createElement('iframe');document.body.appendChild(f)");
        settle(&mut t);
        let child = t.browser.frames(page).into_iter().find(|f| f.parent.is_some()).unwrap();
        weak.push(t.browser.frame(page, &child.id).unwrap().main_realm().unwrap().downgrade());
        eval(&root, "f.remove();f=null");
        t.browser.checkpoint();
        t.browser.engine.collect_garbage_now();
        t.browser.checkpoint();
        assert_eq!(t.browser.frames(page).len(), 1);
    }
    assert!(weak.iter().all(|r| r.upgrade().is_none()));
    t.browser.close_page(page);
    assert!(t.browser.pages.is_empty());
}

#[test]
fn failed_child_fetch_releases_parent_load_without_destroying_initial_document() {
    let mut t = crate::browser_thread().unwrap();
    let loader = Rc::new(StaticLoader::default());
    loader.insert("https://example.test/main", "text/html", "<iframe src='/missing'></iframe>", Duration::ZERO);
    t.browser.set_loader(Some(loader));
    let events = Rc::new(Events::default());
    t.browser.set_observer(Some(events.clone()));
    let page = t.browser.create_page().unwrap();
    t.browser.navigate(page, NavigateOpts::address_bar("https://example.test/main")).unwrap();
    settle(&mut t);
    let frames = t.browser.frames(page);
    assert_eq!(frames.len(), 2);
    assert!(frames.iter().all(|f| f.loaded));
    let child = frames.iter().find(|f| f.parent.is_some()).unwrap();
    assert_eq!(child.url, "about:blank");
    assert!(events
        .0
        .borrow()
        .iter()
        .any(|e| matches!(e, PageEvent::FrameNavigateFailed { frame_id, .. } if frame_id == &child.id)));
}

#[test]
fn inspector_navigation_applies_new_sandbox_and_unnamed_worlds_remain_isolated() {
    let mut t = crate::browser_thread().unwrap();
    let page = t.browser.create_page().unwrap();
    let root = main_realm(&t, page);
    eval(
        &root,
        "globalThis.secret='main'; globalThis.f=document.createElement('iframe'); document.body.appendChild(f)",
    );
    settle(&mut t);
    let child = t.browser.frames(page).into_iter().find(|f| f.parent.is_some()).unwrap();
    let utility = t.browser.isolated_world_in_frame(page, &child.id, "").unwrap();
    assert_eq!(eval(&utility, "typeof parent.secret"), "undefined");
    eval(&root, "f.setAttribute('sandbox','allow-downloads')");
    assert_eq!(eval(&root, "f.contentDocument !== null"), "true");
    t.browser.navigate_frame(page, &child.id, NavigateOpts::address_bar("about:blank")).unwrap();
    let sandboxed = t.browser.frame(page, &child.id).unwrap().main_realm().unwrap().clone();
    assert_eq!(eval(&root, "f.contentDocument === null"), "true");
    eval(&sandboxed, "globalThis.ran=false; setTimeout(()=>ran=true,0);requestAnimationFrame(()=>ran=true)");
    settle(&mut t);
    assert_eq!(eval(&sandboxed, "ran"), "false");
    assert!(utility.is_disposed());
}

#[test]
fn init_script_worlds_exist_in_initial_and_replacement_child_documents() {
    let mut t = crate::browser_thread().unwrap();
    let page = t.browser.create_page().unwrap();
    for (id, world) in [("main", None), ("utility", Some("utility".to_owned()))] {
        t.browser.add_init_script(
            page,
            InitScript {
                id: id.into(),
                world,
                source: format!("globalThis.initWorld='{id}';globalThis.initURL=document.URL"),
            },
        );
    }
    let root = main_realm(&t, page);
    eval(&root, "globalThis.f=document.createElement('iframe');document.body.appendChild(f)");
    let initial = t.browser.frames(page).into_iter().find(|f| f.parent.is_some()).unwrap();
    let before = t.browser.frame(page, &initial.id).unwrap();
    assert_eq!(before.realms.len(), 2);
    for world in &before.realms {
        assert_eq!(eval(&world.realm, "initURL"), "about:blank");
        assert_eq!(eval(&world.realm, "initWorld"), world.world.as_deref().unwrap_or("main"));
    }
    eval(&root, "f.srcdoc='<p>replacement</p>'");
    settle(&mut t);
    let after = t.browser.frame(page, &initial.id).unwrap();
    assert_eq!(after.realms.len(), 2);
    assert!(before.realms.iter().all(|r| r.realm.is_disposed()));
    for world in &after.realms {
        assert_eq!(eval(&world.realm, "initURL"), "about:srcdoc");
        assert_eq!(eval(&world.realm, "initWorld"), world.world.as_deref().unwrap_or("main"));
    }
}
