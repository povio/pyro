use pyro_browser::*;
use pyro_js::Realm;
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

/// A srcdoc document's `location.reload()` loads the iframe's `srcdoc` again (its URL
/// `about:srcdoc` is never fetched); WPT notifications/global-teardown-crash reloads one.
#[test]
fn srcdoc_document_reloads_its_srcdoc() {
    let mut t = crate::browser_thread().unwrap();
    let page = t.browser.create_page().unwrap();
    let root = main_realm(&t, page);
    eval(
        &root,
        "globalThis.loads=0; globalThis.f=document.createElement('iframe'); \
         f.srcdoc='<script>onload=()=>{parent.loads++; if (parent.loads===1) location.reload()}</script>'; \
         document.body.appendChild(f)",
    );
    settle(&mut t);
    assert_eq!(eval(&root, "loads + ':' + f.contentDocument.URL"), "2:about:srcdoc");
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
    // (`name` before insertion: a container's later name change leaves its child's name, as in
    // Chrome since ac2cd04f9.)
    eval(&root,"globalThis.a=document.createElement('iframe');globalThis.b=document.createElement('iframe');a.srcdoc='<p>unused</p>';document.body.appendChild(a);b.name='second';document.body.appendChild(b);a.remove()");
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
        pyro_runtime::RunOutcome::Done
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
            body: b"<iframe></iframe><script>globalThis.late=true</script>".to_vec().into(),
            immutable_local: false,
            content_disposition: None,
            header_csp: None,
            allow_csp_from: None,
            refresh: None,
            content_language: None,
            last_modified: None,
            reporting_endpoints: None, permissions_policy: None,
            connection_allowlist: None,
            link_fetches: Vec::new(),
            integrity_headers: None,
            x_frame_options: None,
            dictionary_links: Vec::new(),
            speculation_rules: Vec::new(),
            no_vary_search: None,
            referrer_policy: None,
            origin_agent_cluster: false, force_load_at_top: false, isolation_policy: 0, ephemeral_jar: false,
            embedder_policy: pyro_net::EmbedderPolicy::UnsafeNone,
            embedder_reporting: Default::default(),
            coop: Vec::new(),
            sent_referrer: None,
            timing: None,
            partial: None,
                nosniff: false, opaque: false,
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
fn failed_child_fetch_releases_parent_load_and_commits_the_error_document() {
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
    // As Chrome, the failed navigation commits the error document (1d848465f; WPT
    // navigating-across-documents/redirect-to-about).
    assert_eq!(child.url, "chrome-error://chromewebdata/");
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

#[test]
fn document_encoding_reaches_urls_and_forms_submitted_into_named_frames() {
    let mut t = crate::browser_thread().unwrap();
    let loader = Rc::new(StaticLoader::default());
    let log = loader.clone();
    loader.insert(
        "https://example.test/main",
        "text/html",
        "<meta charset=euc-jp><iframe name=t></iframe><iframe name=other></iframe>\
         <form id=f action=/r target=t><input type=hidden name=_charset_><input name=q></form>",
        Duration::ZERO,
    );
    // U+3042 is A4 A2 in EUC-JP; U+AC00 has no EUC-JP mapping.
    let submitted = "https://example.test/r?_charset_=EUC-JP&q=%A4%A2%2C%26%2344032%3B";
    let utf8 = "https://example.test/r?_charset_=UTF-8&q=%E3%81%82%2C%EA%B0%80";
    loader.insert(submitted, "text/html", "<p>euc</p>", Duration::ZERO);
    loader.insert(utf8, "text/html", "<p>utf8</p>", Duration::ZERO);
    t.browser.set_loader(Some(loader));
    let page = t.browser.create_page().unwrap();
    t.browser.navigate(page, NavigateOpts::address_bar("https://example.test/main")).unwrap();
    settle(&mut t);
    let root = main_realm(&t, page);
    assert_eq!(eval(&root, "document.characterSet"), "EUC-JP");
    // [Replaceable]: a script's `frames` global shadows the Window attribute.
    assert_eq!(
        eval(&root, "var frames = null; frames = [1]; [frames[0], self === window, parent === window].join()"),
        "1,true,true"
    );
    assert_eq!(
        eval(&root, "globalThis.a=document.createElement('a');a.href='/x?\\u3042\\uac00#\\u3042';a.href"),
        "https://example.test/x?%A4%A2%26%2344032%3B#%E3%81%82"
    );
    assert_eq!(eval(&root, "a.search"), "?%A4%A2%26%2344032%3B");
    // Only ASCII whitespace is stripped, not U+3000.
    assert_eq!(eval(&root, "a.href='/x?\\u3000';a.search"), "?%A1%A1");
    // `new URL` stays UTF-8 whatever the document's encoding.
    assert_eq!(eval(&root, "new URL('https://example.test/?\\u3042').search"), "?%E3%81%82");

    eval(&root, "globalThis.f=document.getElementById('f');f.elements.q.value='\\u3042,\\uac00';f.submit()");
    settle(&mut t);
    let urls = |t: &BrowserThread| -> Vec<String> {
        t.browser.frames(page).into_iter().filter(|f| f.parent.is_some()).map(|f| f.url).collect()
    };
    assert_eq!(urls(&t), vec![submitted.to_owned(), "about:blank".to_owned()]);
    assert_eq!(t.browser.frames(page).iter().filter(|f| f.parent.is_none()).count(), 1);

    // accept-charset beats the document's encoding.
    eval(&root, "f.acceptCharset='bogus utf-8';f.target='other';f.submit()");
    settle(&mut t);
    assert_eq!(urls(&t), vec![submitted.to_owned(), utf8.to_owned()]);
    assert!(log.log.borrow().iter().any(|u| u == utf8));
}

#[test]
fn named_targets_do_not_reach_cross_origin_siblings() {
    let mut t = crate::browser_thread().unwrap();
    let loader = Rc::new(StaticLoader::default());
    let log = loader.clone();
    loader.insert("https://example.test/main", "text/html", "<p>main</p>", Duration::ZERO);
    loader.insert("https://example.test/x?", "text/html", "<p>x</p>", Duration::ZERO);
    t.browser.set_loader(Some(loader));
    let page = t.browser.create_page().unwrap();
    t.browser.navigate(page, NavigateOpts::address_bar("https://example.test/main")).unwrap();
    settle(&mut t);
    let root = main_realm(&t, page);
    eval(
        &root,
        "for (const n of ['a','b']) { const f=document.createElement('iframe');f.name=n;\
         if (n==='a') f.setAttribute('sandbox','allow-scripts');document.body.appendChild(f) }",
    );
    settle(&mut t);
    let frames: Vec<_> = t.browser.frames(page).into_iter().filter(|f| f.parent.is_some()).collect();
    let sandboxed = t.browser.frame(page, &frames[0].id).unwrap();
    let submit = "{ const f=document.createElement('form');f.action='https://example.test/x';f.target='b';\
         document.body.appendChild(f);f.submit() }";
    // Opaque origin: sibling `b` is neither in its subtree nor same-origin.
    eval(sandboxed.main_realm().unwrap(), submit);
    settle(&mut t);
    assert!(!log.log.borrow().iter().any(|u| u.starts_with("https://example.test/x")));
    // The same-origin parent reaches it.
    eval(&root, submit);
    settle(&mut t);
    let after: Vec<_> = t.browser.frames(page).into_iter().filter(|f| f.parent.is_some()).collect();
    assert_eq!(after[1].url, "https://example.test/x?");
}

fn cross_document_page() -> (BrowserThread, PageId, Realm) {
    let mut t = crate::browser_thread().unwrap();
    let page = t.browser.create_page().unwrap();
    let root = main_realm(&t, page);
    eval(&root, "globalThis.f=document.createElement('iframe');f.srcdoc=\"<span id=a data-x=1 style='color:red'>one</span><span>two</span><form><input name=n></form>\";document.body.appendChild(f)");
    settle(&mut t);
    eval(&root, "globalThis.d=f.contentDocument;globalThis.nodes=d.querySelectorAll('span')");
    (t, page, root)
}

#[test]
fn parent_reads_child_nodes_through_collections_and_named_hooks() {
    let (_t, _page, root) = cross_document_page();
    // Indexed and named hooks are called in the caller's realm by JSC.
    assert_eq!(eval(&root, "nodes.length+':'+nodes[0].dataset.x+':'+nodes[1].textContent"), "2:1:two");
    assert_eq!(eval(&root, "nodes[0]===d.getElementById('a') && nodes[0]===nodes.item(0)"), "true");
    assert_eq!(
        eval(
            &root,
            "d.forms[0]===d.querySelector('form') && d.body.children[0]===nodes[0] && d.body.children.a===nodes[0]"
        ),
        "true"
    );
    assert_eq!(
        eval(&root, "nodes[0].attributes.id.value+':'+nodes[0].attributes[0].name+':'+nodes[0].style.color"),
        "a:id:red"
    );
    assert_eq!(eval(&root, "[...nodes].map(n=>n.tagName).join()+':'+Object.keys(nodes).join()"), "SPAN,SPAN:0,1");
    assert_eq!(eval(&root, "nodes[0].dataset.y='2';nodes[0].getAttribute('data-y')+':'+delete nodes[0].dataset.y+':'+nodes[0].hasAttribute('data-y')"), "2:true:false");
    // Wrappers belong to the node document's realm, as in other browsers.
    assert_eq!(
        eval(&root, "nodes[0] instanceof f.contentWindow.HTMLSpanElement && !(nodes[0] instanceof HTMLSpanElement)"),
        "true"
    );
    // An expando is seen from both realms, so there is one wrapper.
    eval(&root, "nodes[0].mark=7");
    assert_eq!(eval(&root, "f.contentWindow.eval(\"document.getElementById('a').mark\")"), "7");
}

#[test]
fn members_of_one_realm_called_on_nodes_of_another_run_in_the_receiver_realm() {
    let (_t, _page, root) = cross_document_page();
    assert_eq!(eval(&root, "Element.prototype.querySelector.call(d.body,'span')===nodes[0]"), "true");
    assert_eq!(
        eval(&root, "Object.getOwnPropertyDescriptor(Node.prototype,'firstChild').get.call(d.body)===nodes[0]"),
        "true"
    );
    assert_eq!(eval(&root, "Document.prototype.createElement.call(d,'b').ownerDocument===d"), "true");
    assert_eq!(eval(&root, "try{Document.prototype.createElement.call(d,'1')}catch(e){String(e instanceof f.contentWindow.DOMException)+':'+e.name}"), "true:InvalidCharacterError");
}

#[test]
fn a_node_keeps_one_wrapper_when_it_moves_between_documents() {
    let (_t, _page, root) = cross_document_page();
    eval(&root, "globalThis.p=document.createElement('p');d.body.append(p)");
    assert_eq!(eval(&root, "d.body.lastChild===p && p.ownerDocument===d && d.querySelector('p')===p"), "true");
    eval(&root, "globalThis.i=d.createElement('i');document.body.append(i)");
    assert_eq!(eval(&root, "document.body.lastChild===i && i.ownerDocument===document && document.querySelector('i')===i && document.adoptNode(i)===i"), "true");
    assert_eq!(
        eval(&root, "globalThis.r=document.createRange();r.selectNodeContents(nodes[0]);r.startContainer===nodes[0]"),
        "true"
    );
}

#[test]
fn listeners_run_across_documents_with_one_event_object() {
    let (_t, _page, root) = cross_document_page();
    eval(&root, "globalThis.seen=[];nodes[0].addEventListener('x',e=>seen.push(e));globalThis.ev=new Event('x',{bubbles:true});nodes[0].dispatchEvent(ev)");
    assert_eq!(eval(&root, "seen.length===1 && seen[0]===ev && ev.target===nodes[0]"), "true");
    // Window of the child, event of the parent.
    eval(&root, "globalThis.wseen=[];f.contentWindow.addEventListener('y',e=>wseen.push(e));globalThis.wev=new Event('y');f.contentWindow.dispatchEvent(wev)");
    assert_eq!(eval(&root, "wseen[0]===wev && wev.target===f.contentWindow"), "true");
    // A parent wrapper moved into the child: child dispatch reaches both realms.
    eval(&root, "globalThis.log=[];globalThis.p=document.createElement('p');p.addEventListener('click',()=>log.push('p'));d.body.append(p);d.body.addEventListener('click',()=>log.push('body'))");
    eval(&root, "f.contentWindow.eval(\"document.querySelector('p').click()\")");
    assert_eq!(eval(&root, "log.join()"), "p,body");
    // A child wrapper moved into the parent: bubbles to the parent's listeners.
    eval(&root, "globalThis.log2=[];document.body.addEventListener('click',()=>log2.push('parent'),{once:true});globalThis.i=d.createElement('i');i.addEventListener('click',()=>log2.push('i'),{once:true});document.body.append(i);i.click();i.click()");
    assert_eq!(eval(&root, "log2.join()"), "i,parent");
}

#[test]
fn cross_document_wrappers_survive_gc_and_the_child_navigating_away() {
    let (mut t, page, root) = cross_document_page();
    eval(&root, "nodes[0].mark=7;nodes[0].addEventListener('x',()=>globalThis.fired=true)");
    eval(&root, "globalThis.first=nodes[0];globalThis.stale=nodes;globalThis.staleDoc=d;delete globalThis.nodes");
    t.browser.engine.collect_garbage_now();
    assert_eq!(eval(&root, "stale[0].mark+':'+(stale[0]===first)"), "7:true");
    let child = t.browser.frames(page).into_iter().find(|f| f.parent.is_some()).unwrap();
    t.browser.navigate_frame(page, &child.id, NavigateOpts::address_bar("about:blank")).unwrap();
    settle(&mut t);
    // The document it left stays usable (HTML: not fully active), its
    // wrappers and their expandos with it.
    assert_eq!(
        eval(&root, "[stale[0] === first, first.mark, stale.length > 0, !!staleDoc.querySelector('span'), staleDoc.defaultView].join()"),
        "true,7,true,true,"
    );
    assert_eq!(eval(&root, "f.contentDocument!==staleDoc && f.contentDocument.URL"), "about:blank");
    t.browser.engine.collect_garbage_now();
}

#[test]
fn sibling_wrappers_stay_inside_their_script_world_and_origin() {
    let (mut t, page, root) = cross_document_page();
    eval(&root, "nodes[0].mark='main'");
    let child = t.browser.frames(page).into_iter().find(|f| f.parent.is_some()).unwrap();
    let root_id = t.browser.frames(page).into_iter().find(|f| f.parent.is_none()).unwrap().id;
    let utility = t.browser.isolated_world_in_frame(page, &root_id, "utility").unwrap();
    let inner = t.browser.isolated_world_in_frame(page, &child.id, "utility").unwrap();
    assert_eq!(
        eval(&utility, "globalThis.u=document.querySelector('iframe').contentDocument.querySelectorAll('span')[0];typeof u.mark+':'+u.dataset.x"),
        "undefined:1"
    );
    eval(&utility, "u.mark='utility'");
    assert_eq!(eval(&inner, "document.getElementById('a').mark"), "utility");
    assert_eq!(eval(&root, "nodes[0].mark"), "main");
}

#[test]
fn content_window_is_one_window_proxy_that_follows_the_frame_to_its_next_document() {
    // JSC patch 0033 (JSGlobalContextAdoptGlobalProxy); an older build keeps a Window per document.
    if !Realm::can_adopt_global_proxy() {
        eprintln!("skipped: this JavaScriptCore lacks JSGlobalContextAdoptGlobalProxy (patch 0033)");
        return;
    }
    let mut t = crate::browser_thread().unwrap();
    let page = t.browser.create_page().unwrap();
    let root = main_realm(&t, page);
    eval(&root, "globalThis.f=document.createElement('iframe');f.srcdoc='<p>one</p><script>globalThis.which=\"one\"</script>';document.body.appendChild(f)");
    settle(&mut t);
    let child = t.browser.frames(page).into_iter().find(|f| f.parent.is_some()).unwrap();
    let old = t.browser.frame(page, &child.id).unwrap().main_realm().unwrap().clone();
    assert_eq!(
        eval(&root, "globalThis.held=f.contentWindow;globalThis.heard=[];globalThis.listener=()=>heard.push('old');held.addEventListener('ping',listener);held.which"),
        "one"
    );
    eval(&root, "f.srcdoc='<p>two</p><script>globalThis.which=\"two\";globalThis.me=window</script>'");
    settle(&mut t);
    let new = t.browser.frame(page, &child.id).unwrap().main_realm().unwrap().clone();
    assert!(old.is_disposed());
    // The reference kept from the first document reaches the second one's global, as the same object.
    assert_eq!(
        eval(&root, "[held===f.contentWindow, held.which, held.document.querySelector('p').textContent, held.me===held, frames[0]===held, held.self===held, held.window===held, held.parent===window].join()"),
        "true,two,two,true,true,true,true,true"
    );
    // Its methods are the new Window's; a listener the old document's Window had is gone with it.
    assert_eq!(
        eval(&root, "held.removeEventListener('ping',listener);held.dispatchEvent(new Event('ping'));held.addEventListener('ping',()=>heard.push('new'));held.dispatchEvent(new Event('ping'));heard.join()+':'+held.location.href"),
        "new:about:srcdoc"
    );
    assert_eq!(
        eval(&new, "[window===parent.f.contentWindow, globalThis===window, this===window, self===window].join()"),
        "true,true,true,true"
    );
    t.browser.engine.collect_garbage_now();
    assert_eq!(eval(&root, "held.which"), "two");
    // Another frame is another browsing context, with a WindowProxy of its own.
    eval(&root, "globalThis.g=document.createElement('iframe');document.body.appendChild(g)");
    settle(&mut t);
    assert_eq!(eval(&root, "g.contentWindow!==held && g.contentWindow===frames[1]"), "true");
}

/// WPT html/semantics/permission-element/geolocation-element/iframe-use-after-free-crash.html:
/// a document its frame navigated away from stays usable through a reference
/// script saved (HTML: it is only not fully active), and its realm goes once
/// nothing reaches it.
#[test]
fn a_navigated_away_document_stays_usable_while_held() {
    let mut t = crate::browser_thread().unwrap();
    let page = t.browser.create_page().unwrap();
    let root = main_realm(&t, page);
    eval(&root, "globalThis.f=document.createElement('iframe'); f.srcdoc='<p id=old>old</p>'; document.body.appendChild(f)");
    settle(&mut t);
    eval(&root, "globalThis.saved = f.contentDocument; globalThis.savedWindow = f.contentWindow");
    eval(&root, "f.srcdoc='<p id=new>new</p>'");
    settle(&mut t);
    assert_eq!(eval(&root, "f.contentDocument.querySelector('p').id + ':' + (f.contentDocument !== saved)"), "new:true");
    assert_eq!(
        eval(
            &root,
            "(() => { const g = saved.createElement('geolocation'); g.setAttribute('watch', ''); saved.body.appendChild(g); \
             return [saved.querySelector('p').id, saved.body.lastChild === g, g.ownerDocument === saved, saved.defaultView, \
              saved.body.childElementCount].join() })()"
        ),
        "old,true,true,,2"
    );
    let engine = &t.browser.engine;
    for _ in 0..3 {
        engine.collect_garbage_now();
        engine.drain_finalized();
    }
    assert_eq!(engine.lingering_realms(), 1, "the saved document keeps its realm");
    eval(&root, "saved = null; savedWindow = null");
    for _ in 0..5 {
        engine.collect_garbage_now();
        engine.drain_finalized();
    }
    assert_eq!(engine.lingering_realms(), 0, "nothing reaches the old document: its realm is disposed");
}

/// A removed frame's window that script still holds lingers with its realm, and a Location made
/// for it then still gets its [LegacyUnforgeable] own members: the function installing them is a
/// root of the realm's own, handed to its global when it lingers, not one of the window's that
/// kept every lingering global alive.
#[test]
fn a_lingering_windows_location_keeps_its_own_members() {
    let mut t = crate::browser_thread().unwrap();
    let page = t.browser.create_page().unwrap();
    let root = main_realm(&t, page);
    eval(&root, "globalThis.f=document.createElement('iframe'); f.srcdoc='<p>x</p>'; document.body.appendChild(f)");
    settle(&mut t);
    eval(&root, "globalThis.w = f.contentWindow; f.remove()");
    settle(&mut t);
    for _ in 0..3 {
        t.browser.engine.collect_garbage_now();
        t.browser.engine.drain_finalized();
    }
    assert_eq!(
        eval(&root, "[typeof w.location.replace, typeof w.location.href, Object.hasOwn(w.location, 'assign')].join()"),
        "function,string,true"
    );
}

/// The documents an iframe navigates through linger only while something
/// reaches them: after 50 navigations nobody holds, none is left (fork mode
/// and long e2e runs keep one engine for many pages).
#[test]
fn documents_of_a_navigating_frame_are_released() {
    let mut t = crate::browser_thread().unwrap();
    let page = t.browser.create_page().unwrap();
    let root = main_realm(&t, page);
    eval(&root, "globalThis.f=document.createElement('iframe'); f.srcdoc='<p>0</p>'; document.body.appendChild(f)");
    settle(&mut t);
    for i in 1..=50 {
        eval(
            &root,
            &format!(
                "f.srcdoc='<p id=p{i}>{i}</p><script>globalThis.big = new Array(10000).fill(document.body); \
                 document.addEventListener(\"click\", () => big); requestAnimationFrame(() => big)</script>'"
            ),
        );
        settle(&mut t);
    }
    assert_eq!(eval(&root, "f.contentDocument.querySelector('p').id"), "p50");
    let engine = &t.browser.engine;
    for _ in 0..5 {
        engine.collect_garbage_now();
        engine.drain_finalized();
    }
    assert_eq!(engine.lingering_realms(), 0, "every previous document was released");
}

/// IntersectionObserver across documents: the parent observes an element of
/// its iframe's document (implicit root: the top-level viewport). The
/// target is clipped by the iframe's viewport and placed through the owner
/// element; the entry reports it in the target document's coordinates. A
/// removed frame's target stops intersecting (WPT
/// image-loading-lazy-subframe-detached-crash.html removes it from the
/// callback).
#[test]
fn intersection_observer_sees_targets_in_child_documents() {
    let mut t = crate::browser_thread().unwrap();
    let page = t.browser.create_page().unwrap();
    let root = main_realm(&t, page);
    eval(
        &root,
        "document.body.style.margin='0'; globalThis.f=document.createElement('iframe'); \
         f.style.cssText='position:absolute; left:100px; top:50px; width:200px; height:100px; border:0'; \
         f.srcdoc='<body style=\"margin:0\"><div id=near style=\"height:50px;width:50px\"></div>\
           <div style=\"height:500px\"></div><div id=far style=\"height:50px;width:50px\"></div></body>'; \
         document.body.appendChild(f)",
    );
    settle(&mut t);
    eval(
        &root,
        "globalThis.seen=[]; globalThis.io=new IntersectionObserver(es => { for (const e of es) \
           seen.push([e.target.id, e.isIntersecting, e.intersectionRatio, e.intersectionRect.x, e.intersectionRect.y, \
             e.rootBounds.width].join(':')) }); \
         io.observe(f.contentDocument.getElementById('near')); io.observe(f.contentDocument.getElementById('far'))",
    );
    settle(&mut t);
    assert_eq!(eval(&root, "seen.sort().join(' ')"), "far:false:0:0:0:1920 near:true:1:0:0:1920");
    // Scrolled to the end (500 px), `far` intersects; the owner's
    // position does not leak into the target document's coordinates.
    eval(&root, "seen.length=0; f.contentWindow.scrollTo(0, 520); io.takeRecords(); document.body.offsetTop");
    eval(&root, "document.body.appendChild(document.createElement('p'))");
    settle(&mut t);
    assert_eq!(eval(&root, "seen.sort().join(' ')"), "far:true:1:0:50:1920 near:false:0:0:0:1920");
    // The frame goes: its targets intersect nothing.
    eval(&root, "seen.length=0; f.remove()");
    settle(&mut t);
    assert_eq!(eval(&root, "seen.map(s => s.split(':').slice(0, 2).join(':')).sort().join(' ')"), "far:false");
}

/// A `Content-Security-Policy` header's Trusted Types directives apply to the document
/// (`require-trusted-types-for 'script'`: string HTML assignments throw), next to its other
/// directives (`script-src 'unsafe-inline'` lets the inline script run).
#[test]
fn csp_header_trusted_types_directives_apply_others_do_not() {
    let mut t = crate::browser_thread().unwrap();
    let page = t.browser.create_page().unwrap();
    let loader = Rc::new(pyro_browser::StaticLoader::default());
    loader.insert(
        "http://site.test/",
        "text/html",
        "<script>globalThis.out = []; const d = document.createElement('div'); \
         try { d.innerHTML = '<b>x</b>'; out.push('assigned'); } catch (e) { out.push(e.name); } \
         try { trustedTypes.createPolicy('other', {}); out.push('created'); } catch (e) { out.push(e.name); } \
         const p = trustedTypes.createPolicy('p', { createHTML: s => s }); d.innerHTML = p.createHTML('<i>y</i>'); out.push(d.innerHTML);</script>",
        Duration::ZERO,
    );
    loader.set_csp("http://site.test/", "script-src 'unsafe-inline'; require-trusted-types-for 'script'; trusted-types p");
    t.browser.set_loader(Some(loader.clone()));
    t.browser.navigate(page, NavigateOpts::address_bar("http://site.test/")).unwrap();
    settle(&mut t);
    let root = main_realm(&t, page);
    assert_eq!(eval(&root, "out.join('|')"), "TypeError|TypeError|<i>y</i>");
}

/// CSP `script-src 'none'` (a header) blocks the document's inline script.
#[test]
fn csp_header_script_src_none_blocks_inline_scripts() {
    let mut t = crate::browser_thread().unwrap();
    let page = t.browser.create_page().unwrap();
    let loader = Rc::new(pyro_browser::StaticLoader::default());
    loader.insert("http://site.test/", "text/html", "<script>globalThis.ran = true</script>", Duration::ZERO);
    loader.set_csp("http://site.test/", "script-src 'none'");
    t.browser.set_loader(Some(loader.clone()));
    t.browser.navigate(page, NavigateOpts::address_bar("http://site.test/")).unwrap();
    settle(&mut t);
    let root = main_realm(&t, page);
    assert_eq!(eval(&root, "String(globalThis.ran)"), "undefined");
}

/// CSP's inline check is part of "prepare the script element" (at its end tag): an inline
/// script a stylesheet holds is checked with the nonce it had there, not one a script changed
/// while it waited (WPT content-security-policy/script-src/scriptnonce-changed-1).
#[test]
fn csp_inline_script_held_by_a_stylesheet_keeps_its_end_tag_nonce() {
    let mut t = crate::browser_thread().unwrap();
    let page = t.browser.create_page().unwrap();
    let loader = Rc::new(pyro_browser::StaticLoader::default());
    loader.insert(
        "http://site.test/",
        "text/html",
        "<link rel=stylesheet href=/slow.css><script src=/change.js async></script>\
         <script id=s nonce=abc>globalThis.ran = true</script>",
        Duration::ZERO,
    );
    loader.insert("http://site.test/slow.css", "text/css", "p {}", Duration::from_millis(100));
    loader.insert(
        "http://site.test/change.js",
        "text/javascript",
        "globalThis.changed = true; document.getElementById('s').setAttribute('nonce', 'zzz')",
        Duration::from_millis(10),
    );
    loader.set_csp("http://site.test/", "script-src 'self' 'nonce-abc'");
    t.browser.set_loader(Some(loader.clone()));
    t.browser.navigate(page, NavigateOpts::address_bar("http://site.test/")).unwrap();
    settle(&mut t);
    let root = main_realm(&t, page);
    assert_eq!(eval(&root, "[globalThis.changed, globalThis.ran].join()"), "true,true");
}

/// CSP `'self'` in a sandboxed (opaque-origin) document matches its precursor origin, the one
/// it would have without the sandbox, as Chrome (WPT content-security-policy/meta/sandbox-iframe).
#[test]
fn csp_self_matches_the_precursor_origin_of_a_sandboxed_document() {
    let mut t = crate::browser_thread().unwrap();
    let page = t.browser.create_page().unwrap();
    let loader = Rc::new(pyro_browser::StaticLoader::default());
    loader.insert(
        "http://site.test/",
        "text/html",
        "<script>globalThis.got = 'none'; addEventListener('message', e => { globalThis.got = String(e.data) })</script>\
         <iframe sandbox=allow-scripts src=/child></iframe>",
        Duration::ZERO,
    );
    loader.insert(
        "http://site.test/child",
        "text/html",
        "<meta http-equiv=Content-Security-Policy content=\"script-src 'self' 'unsafe-inline'\">\
         <script src=/s.js></script><script>parent.postMessage(String(globalThis.loaded), '*')</script>",
        Duration::ZERO,
    );
    loader.insert("http://site.test/s.js", "text/javascript", "globalThis.loaded = 1", Duration::ZERO);
    t.browser.set_loader(Some(loader.clone()));
    t.browser.navigate(page, NavigateOpts::address_bar("http://site.test/")).unwrap();
    settle(&mut t);
    let root = main_realm(&t, page);
    assert_eq!(eval(&root, "globalThis.got"), "1");
}

/// SVG `<use>` instances: a closed user-agent shadow root with a deep clone
/// of the referenced element, rebuilt when the reference or its subtree
/// changes; nothing of it is reachable from script; an SVG handler's
/// argument is `evt`.
#[test]
fn svg_use_elements_get_live_instances() {
    let mut t = crate::browser_thread().unwrap();
    let page = t.browser.create_page().unwrap();
    let root = main_realm(&t, page);
    eval(
        &root,
        "document.body.innerHTML = '<svg><defs><g id=t><rect id=r width=10 height=10 onclick=\"window.arg = typeof evt\"/></g>\
           <circle id=c r=5 /></defs><use id=u href=\"#t\"/></svg>'; 'ok'",
    );
    settle(&mut t);
    let instance = |t: &BrowserThread| {
        let dom = t.browser.page(page).unwrap().main_frame.page_dom.clone().unwrap();
        let d = dom.dom.borrow();
        let doc = dom.document();
        let host = d.get_element_by_id(doc, "u").unwrap().unwrap();
        d.shadow_root(host).unwrap().map(|sr| {
            d.children(sr).unwrap().map(|c| d.outer_html(c).unwrap()).collect::<Vec<_>>().join("")
        })
    };
    assert_eq!(instance(&t).as_deref(), Some(r#"<g id="t"><rect id="r" width="10" height="10" onclick="window.arg = typeof evt"></rect></g>"#));
    assert_eq!(eval(&root, "[u.shadowRoot, document.getElementById('r').getRootNode() === document].join()"), ",true");
    // The referenced subtree changes: the instance follows after the task.
    eval(&root, "document.getElementById('r').setAttribute('width', '20'); 'ok'");
    settle(&mut t);
    assert!(instance(&t).unwrap().contains(r#"width="20""#));
    // Another reference, then none.
    eval(&root, "u.setAttribute('href', '#c'); 'ok'");
    settle(&mut t);
    assert_eq!(instance(&t).as_deref(), Some(r#"<circle id="c" r="5"></circle>"#));
    eval(&root, "u.setAttribute('href', '#missing'); 'ok'");
    settle(&mut t);
    assert_eq!(instance(&t).as_deref(), Some(""));
    // A cycle makes no instance.
    eval(&root, "document.getElementById('t').append(u); u.setAttribute('href', '#t'); 'ok'");
    settle(&mut t);
    assert_eq!(instance(&t).as_deref(), Some(""));
    // SVG handlers take `evt`.
    eval(&root, "document.getElementById('r').dispatchEvent(new Event('click')); 'ok'");
    assert_eq!(eval(&root, "arg"), "object");
}

/// SVG `<image>` fires `error` for an `href` that fetches no decodable image
/// (WPT svg/interact/image-load-error-events.html covers `load`); the clone in a `<use>` instance loads too
/// and its handler sees the clone (WPT css/selectors/crashtests/chrome-bug-565742178.html).
#[test]
fn svg_images_fire_error_in_use_instances_too() {
    let mut t = crate::browser_thread().unwrap();
    let page = t.browser.create_page().unwrap();
    let root = main_realm(&t, page);
    eval(
        &root,
        "globalThis.log = []; document.body.innerHTML = '<svg><defs><g id=t>\\
         <image href=\"data:image/png;base64,AAAA\" onerror=\"log.push(\\'error:\\' + (evt.target.getRootNode() === document))\"/></g></defs>\\
         <use href=\"#t\"/></svg>'; 'ok'",
    );
    settle(&mut t);
    let mut log: Vec<String> = eval(&root, "log.join()").split(',').map(str::to_owned).collect();
    log.sort();
    assert_eq!(log, ["error:false", "error:true"]);
}

/// An `<object>` whose `data` is a document (`type="text/html"`, or an HTML file extension) has a
/// child navigable as an iframe does: the document loads and scripts, `contentDocument` and
/// `contentWindow` reach it, the element gets `load`; other objects keep none.
#[test]
fn an_object_showing_a_document_is_a_frame() {
    let mut t = crate::browser_thread().unwrap();
    let page = t.browser.create_page().unwrap();
    let loader = Rc::new(pyro_browser::StaticLoader::default());
    loader.insert(
        "http://site.test/",
        "text/html",
        "<object id=a type='text/html' data='/inner.html'>fallback</object><object id=b data='/inner.htm'></object>\
         <object id=c data='/pic.png'>img</object><object id=d type='text/html'>no data</object>\
         <script>globalThis.loads = 0; for (const id of ['a', 'b']) document.getElementById(id).onload = () => loads++;</script>",
        Duration::ZERO,
    );
    loader.insert("http://site.test/inner.html", "text/html", "<p id=p>inner</p><script>window.answer = 42</script>", Duration::ZERO);
    loader.insert("http://site.test/inner.htm", "text/html", "<p id=p>second</p>", Duration::ZERO);
    t.browser.set_loader(Some(loader.clone()));
    t.browser.navigate(page, NavigateOpts::address_bar("http://site.test/")).unwrap();
    settle(&mut t);
    let root = main_realm(&t, page);
    assert_eq!(
        eval(
            &root,
            "const o = id => document.getElementById(id); \
             [loads, o('a').contentDocument.getElementById('p').textContent, o('a').contentWindow.answer, \
              o('a').contentWindow.parent === window, o('b').contentDocument.getElementById('p').textContent, \
              o('c').contentDocument, o('d').contentWindow, window.length].join('|')"
        ),
        "2|inner|42|true|second|||2"
    );
}

/// HTML "the object element" processing: without a document `type` or URL, the response's
/// Content-Type decides (an extensionless URL served as HTML, or `type` saying an image of an
/// HTML response, is a document); `data`/`type` changes run the steps again (a child navigable
/// comes, navigates or goes); an `<embed>` with an HTML `src` shows the document too.
#[test]
fn object_content_type_and_attribute_changes_decide_its_navigable() {
    let mut t = crate::browser_thread().unwrap();
    let page = t.browser.create_page().unwrap();
    let loader = Rc::new(pyro_browser::StaticLoader::default());
    loader.insert(
        "http://site.test/",
        "text/html",
        "<object id=a data='/page'>fallback</object><object id=b type='image/png' data='/page'></object>\
         <object id=c data='/pic.png'></object><embed id=e src='/doc.html'>\
         <script>globalThis.loads = 0; for (const id of ['a', 'b']) document.getElementById(id).onload = () => loads++;</script>",
        Duration::ZERO,
    );
    loader.insert("http://site.test/page", "text/html", "<p id=p>page</p>", Duration::ZERO);
    loader.insert("http://site.test/doc.html", "text/html", "<p id=p>doc</p>", Duration::ZERO);
    loader.insert("http://site.test/other.html", "text/html", "<p id=p>other</p>", Duration::ZERO);
    loader.insert("http://site.test/pic.png", "image/png", "not really a png", Duration::ZERO);
    t.browser.set_loader(Some(loader.clone()));
    t.browser.navigate(page, NavigateOpts::address_bar("http://site.test/")).unwrap();
    settle(&mut t);
    let root = main_realm(&t, page);
    assert_eq!(
        eval(
            &root,
            "globalThis.o = id => document.getElementById(id); globalThis.text = id => o(id).contentDocument?.getElementById('p')?.textContent ?? 'none'; \
             globalThis.frameTexts = () => [...Array(window.length)].map((_, i) => window[i].document.getElementById('p')?.textContent).sort().join(','); \
             [loads, text('a'), text('b'), text('c'), frameTexts()].join('|')"
        ),
        "2|page|page|none|doc,page,page"
    );
    // `c` turns into a document object, `a` loses its document, `b` navigates elsewhere.
    eval(&root, "o('c').type = 'text/html'; o('c').data = '/other.html'; o('a').data = '/pic.png'; o('b').data = '/doc.html';");
    settle(&mut t);
    assert_eq!(eval(&root, "[text('a'), text('b'), text('c'), frameTexts()].join('|')"), "none|doc|other|doc,doc,other");
}

/// An `<object>` is fetched first even when its `type` says text/html: a failed fetch fires
/// `error` and leaves the fallback (no child navigable); one not being rendered is not fetched
/// until it is, then shows its document.
#[test]
fn object_fallback_on_failure_and_document_once_rendered() {
    let mut t = crate::browser_thread().unwrap();
    let page = t.browser.create_page().unwrap();
    let loader = Rc::new(pyro_browser::StaticLoader::default());
    loader.insert(
        "http://site.test/",
        "text/html",
        "<object id=missing type='text/html' data='/missing.html'>fallback</object>\
         <object id=hidden style='display:none' data='/doc.html'></object>\
         <script>globalThis.events = []; for (const id of ['missing', 'hidden']) for (const t of ['load', 'error']) \
           document.getElementById(id).addEventListener(t, () => events.push(id + ':' + t));</script>",
        Duration::ZERO,
    );
    loader.insert("http://site.test/doc.html", "text/html", "<p id=p>doc</p>", Duration::ZERO);
    t.browser.set_loader(Some(loader.clone()));
    t.browser.navigate(page, NavigateOpts::address_bar("http://site.test/")).unwrap();
    settle(&mut t);
    let root = main_realm(&t, page);
    assert_eq!(
        eval(&root, "globalThis.o = id => document.getElementById(id); [events.join(), o('missing').contentDocument, o('hidden').contentDocument, window.length].join('|')"),
        "missing:error|||0"
    );
    eval(&root, "o('hidden').style.display = 'inline';");
    settle(&mut t);
    assert_eq!(
        eval(&root, "[events.join(), o('hidden').contentDocument?.getElementById('p')?.textContent, window.length].join('|')"),
        "missing:error,hidden:load|doc|1"
    );
}

/// HTML Window reuse: an iframe's first document of its own origin keeps
/// the Window of its initial about:blank document, so what the parent put on
/// `contentWindow` right after inserting the iframe (expandos, listeners,
/// `onload`) is there for the loaded document. A second navigation gets a
/// new Window (another origin does too, see
/// `child_location_navigation_keeps_parent_and_sibling_realms`).
#[test]
fn first_same_origin_document_reuses_the_initial_window() {
    let mut t = crate::browser_thread().unwrap();
    let loader = Rc::new(StaticLoader::default());
    loader.insert("https://example.test/main", "text/html", "<body></body>", Duration::ZERO);
    loader.insert(
        "https://example.test/child",
        "text/html",
        "<p id=c>child</p><script>window.seenByChild = typeof window.marker; addEventListener('load', () => parent.order.push('child-listener'))</script>",
        Duration::from_millis(10),
    );
    loader.insert("https://example.test/next", "text/html", "<p id=n>next</p>", Duration::ZERO);
    t.browser.set_loader(Some(loader));
    let page = t.browser.create_page().unwrap();
    t.browser.navigate(page, NavigateOpts::address_bar("https://example.test/main")).unwrap();
    settle(&mut t);
    let root = main_realm(&t, page);
    eval(
        &root,
        "globalThis.order = []; globalThis.f = document.createElement('iframe'); f.src = '/child'; \
         document.body.appendChild(f); globalThis.w = f.contentWindow; globalThis.blankDoc = w.document; \
         w.marker = 'set'; w.addEventListener('load', () => order.push('parent-listener')); \
         w.onload = () => order.push('onload');",
    );
    settle(&mut t);
    assert_eq!(eval(&root, "f.contentDocument.getElementById('c').textContent"), "child");
    assert_eq!(eval(&root, "f.contentWindow === w"), "true");
    assert_eq!(eval(&root, "String(w.marker) + ' ' + w.seenByChild"), "set string");
    assert_eq!(eval(&root, "w.document === f.contentDocument && w.document !== blankDoc"), "true");
    assert_eq!(eval(&root, "order.join(',')"), "parent-listener,onload,child-listener");
    // A second navigation of the reusing frame makes a new Window.
    eval(&root, "f.src = '/next'");
    settle(&mut t);
    assert_eq!(
        eval(&root, "f.contentDocument.getElementById('n').textContent + ' ' + String(w.marker)"),
        "next undefined"
    );
}

/// The Window an iframe's first same-origin document reuses keeps the
/// per-realm bindings install.js made for its initial about:blank document.
/// Its Blob and File constructors read their parts (they made empty blobs,
/// so a test runner's `new File([xml])` uploaded nothing), and the other
/// helper-backed APIs work there as they do in a new Window.
#[test]
fn reused_window_keeps_its_blob_and_helper_bindings() {
    let mut t = crate::browser_thread().unwrap();
    let loader = Rc::new(StaticLoader::default());
    loader.insert("https://example.test/main", "text/html", "<body></body>", Duration::ZERO);
    let probe = "[new Blob(['abc']).size, new File([new Uint8Array(4)], 'f.xml', { type: 'text/xml' }).size, \
                 new Blob([new Blob(['xy']), 'z\\r\\n'], { endings: 'native' }).size, \
                 new TextDecoder('latin1').decode(new Uint8Array([0xe9])), new Headers({ a: '1' }).get('a'), \
                 new URLSearchParams('q=1&q=2').getAll('q').join('+'), structuredClone({ n: 5 }).n].join(',')";
    loader.insert(
        "https://example.test/child",
        "text/html",
        &format!("<script>window.made = {probe}</script>"),
        Duration::from_millis(10),
    );
    loader.insert(
        "https://example.test/next",
        "text/html",
        &format!("<script>window.made = {probe}</script>"),
        Duration::ZERO,
    );
    t.browser.set_loader(Some(loader));
    let page = t.browser.create_page().unwrap();
    t.browser.navigate(page, NavigateOpts::address_bar("https://example.test/main")).unwrap();
    settle(&mut t);
    let root = main_realm(&t, page);
    eval(
        &root,
        "globalThis.f = document.createElement('iframe'); f.src = '/child'; document.body.appendChild(f); \
         globalThis.w = f.contentWindow",
    );
    settle(&mut t);
    assert_eq!(eval(&root, "f.contentWindow === w"), "true", "the child document reuses the initial Window");
    let expected = "3,4,4,é,1,1+2,5";
    assert_eq!(eval(&root, "w.made"), expected);
    // Constructed from the parent through the reused Window, and through its eval.
    assert_eq!(
        eval(
            &root,
            "[new w.Blob(['abcd']).size, new w.File(['ab'], 'a').size, \
             w.eval(\"new Blob([new Uint8Array(6)]).size\")].join()"
        ),
        "4,2,6"
    );
    // A new Window (the second navigation) has them too.
    eval(&root, "f.src = '/next'");
    settle(&mut t);
    assert_eq!(eval(&root, "f.contentWindow.made"), expected);
}

/// A frame document is given to the renderer at its own base URL (its `<base href>`), as the top
/// document's capture is (security review cycle 5, 5).
#[test]
fn a_frame_document_is_rendered_at_its_own_base_url() {
    let mut t = crate::browser_thread().unwrap();
    let page = t.browser.create_page().unwrap();
    let root = main_realm(&t, page);
    eval(&root, "globalThis.f=document.createElement('iframe'); f.srcdoc='<base href=\"http://other.test/dir/\"><p>x</p>'; document.body.appendChild(f); 'ok'");
    settle(&mut t);
    let p = t.browser.page(page).unwrap();
    p.update_frame_documents();
    let dom = p.main_frame.page_dom.clone().unwrap();
    let table = dom.dom.borrow().frame_table(dom.document());
    let urls: Vec<String> = table.entries.iter().map(|(_, e)| e.url.clone()).collect();
    assert_eq!(urls, vec!["http://other.test/dir/".to_string()]);
}

/// A render snapshot of the top document in markup form (the `capture_only` snapshot), rendered
/// as a fresh document, paints a scrolled page's `position: sticky` box where the page holds it:
/// the shift travels in its `data-pyro-sticky` attribute, which the renderer applies (the
/// renderer lays sticky boxes out as static). Chrome: scrolled 100px, the box's top is 0. (Cycle
/// 8b: the old top-document path took scroll offsets only and painted it 50px above the
/// viewport.)
#[test]
fn a_markup_snapshot_paints_a_stuck_sticky_box_where_the_page_holds_it() {
    struct NoFetch;
    impl pyro_render::Fetcher for NoFetch {
        fn fetch(&self, _url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
            done(Err("none".into()));
        }
    }
    let mut t = crate::browser_thread().unwrap();
    let page = t.browser.create_page().unwrap();
    let root = main_realm(&t, page);
    eval(
        &root,
        "document.body.style.margin = '0'; document.body.innerHTML = \"<div style='height:50px'></div>\
         <div id=s style='position:sticky;top:0;height:20px;background:rgb(0,128,0)'></div>\
         <div style='height:2000px;background:rgb(0,0,255)'></div>\"; scrollTo(0, 100); 'ok'",
    );
    settle(&mut t);
    assert_eq!(eval(&root, "[scrollY, document.getElementById('s').getBoundingClientRect().top].join()"), "100,0");
    let dom = t.browser.page(page).unwrap().main_frame.page_dom.clone().unwrap();
    let s = dom.render_snapshot("about:blank", None);
    assert!(s.html.contains(&format!("data-pyro-{}-sticky", s.nonce)), "{}", s.html);
    let snapshot = pyro_render::Snapshot {
        html: s.html,
        base_url: "about:blank".into(),
        sheets: s.sheets,
        width: s.width,
        height: s.height,
        device_scale_factor: 1.0,
        scroll_x: s.scroll_x,
        scroll_y: s.scroll_y,
        // (The nonce its engine attributes are named with, as a `capture_only` snapshot carries it.)
        frames: std::sync::Arc::new(pyro_render::FrameTableData { nonce: s.nonce, entries: Default::default() }),
        ..Default::default()
    };
    let fetcher: std::sync::Arc<dyn pyro_render::Fetcher> = std::sync::Arc::new(NoFetch);
    let (image, _, _) =
        pyro_render::Renderer::new().render(&snapshot, pyro_render::Area::Viewport, pyro_render::Backend::Cpu, &fetcher).unwrap();
    let at = |x: u32, y: u32| {
        let i = ((y * image.width + x) * 4) as usize;
        image.data[i..i + 3].to_vec()
    };
    assert_eq!((at(5, 5), at(5, 15), at(5, 30)), (vec![0, 128, 0], vec![0, 128, 0], vec![0, 0, 255]));
}

#[test]
fn cookie_partition_and_site_for_cookies_follow_the_whole_frame_chain() {
    use pyro_net::cookies::SiteContext::{CrossSite, SameSite};

    let mut t = crate::browser_thread().unwrap();
    let loader = Rc::new(StaticLoader::default());
    let top_url = "https://app.example.test/page";
    let sibling_url = "https://static.example.test/sibling";
    let cross_url = "https://other.test/middle";
    let nested_url = "https://app.example.test/leaf";
    loader.insert(top_url, "text/html", &format!("<iframe src='{sibling_url}'></iframe><iframe src='{cross_url}'></iframe>"), Duration::ZERO);
    loader.insert(cross_url, "text/html", &format!("<iframe src='{nested_url}'></iframe>"), Duration::ZERO);
    for url in [nested_url, sibling_url] {
        loader.insert(url, "text/html", "<p>loaded</p>", Duration::ZERO);
    }
    t.browser.set_loader(Some(loader));
    let page = t.browser.create_page().unwrap();
    t.browser.navigate(page, NavigateOpts::address_bar(top_url)).unwrap();
    settle(&mut t);
    let frames = t.browser.frames(page);
    for (url, expected) in [(top_url, SameSite), (sibling_url, SameSite), (cross_url, CrossSite), (nested_url, CrossSite)] {
        let frame = frames.iter().find(|f| f.url == url).unwrap();
        let document = t.browser.frame(page, &frame.id).unwrap().page_dom.unwrap();
        assert_eq!(document.cookie_partition_key().as_deref(), Some("https://example.test"), "{url}");
        assert_eq!(document.cookie_site_context(), expected, "{url}");
    }
}

/// css-sizing-4 responsive iframes: a script of the embedder reads an iframe of `frame-sizing:
/// content-height` (`content-width`, a logical value by the iframe's writing mode) at the content
/// size of a document that opted in with `<meta name="responsive-embedded-sizing">` admitting the
/// embedder's origin, and at the default 300x150 otherwise. `window.requestResize()` throws
/// `NotAllowedError` outside such a document. (WPT css-sizing `responsive-iframe/responsive-iframe-value`,
/// `-allow-origins`, `-request-resize-error`.)
#[test]
fn an_embedders_script_reads_a_responsive_iframe_at_its_content_size() {
    let mut t = crate::browser_thread().unwrap();
    let loader = Rc::new(StaticLoader::default());
    loader.insert("https://example.test/main", "text/html", "<body></body>", Duration::ZERO);
    let child = |allow: &str| {
        format!(
            "<!doctype html><style>* {{ margin: 0 }} :root {{ width: 400px; height: 200px }}</style>\
             <meta name=responsive-embedded-sizing content=\"{allow}\"><body>\
             <script>try {{ requestResize(); window.resized = 'ok' }} catch (e) {{ window.resized = e.name }}</script>"
        )
    };
    loader.insert("https://example.test/any", "text/html", &child("allow-origins=*"), Duration::ZERO);
    loader.insert("https://example.test/parent", "text/html", &child("allow-origins=https://example.test"), Duration::ZERO);
    loader.insert("https://example.test/other", "text/html", &child("allow-origins=https://other.test"), Duration::ZERO);
    loader.insert("https://example.test/prefix", "text/html", &child("ALLOW-ORIGINS=*"), Duration::ZERO);
    loader.insert(
        "https://example.test/plain",
        "text/html",
        "<!doctype html><style>:root { height: 200px }</style><body><script>try { requestResize(); window.resized = 'ok' } catch (e) { window.resized = e.name }</script>",
        Duration::ZERO,
    );
    t.browser.set_loader(Some(loader));
    let page = t.browser.create_page().unwrap();
    t.browser.navigate(page, NavigateOpts::address_bar("https://example.test/main")).unwrap();
    settle(&mut t);
    let root = main_realm(&t, page);
    eval(
        &root,
        "globalThis.frames_ = {}; \
         for (const [name, sizing, wm] of [['any', 'content-height'], ['parent', 'content-height'], \
              ['other', 'content-height'], ['prefix', 'content-height'], ['plain', 'content-height'], \
              ['width', 'content-width'], ['inline', 'content-inline-size', 'vertical-rl']]) { \
           const f = document.createElement('iframe'); f.frameBorder = '0'; \
           f.style.setProperty('frame-sizing', sizing); if (wm) f.style.writingMode = wm; \
           f.src = ['width', 'inline'].includes(name) ? '/any' : '/' + name; \
           document.body.appendChild(f); frames_[name] = f; } 'ok'",
    );
    settle(&mut t);
    let size = |name: &str| eval(&root, &format!("[frames_.{name}.offsetWidth, frames_.{name}.offsetHeight].join()"));
    assert_eq!(size("any"), "300,200");
    assert_eq!(size("parent"), "300,200");
    assert_eq!(size("other"), "300,150", "the list does not admit the embedder");
    assert_eq!(size("prefix"), "300,150", "`allow-origins=` is case-sensitive");
    assert_eq!(size("plain"), "300,150", "not responsively embedded");
    assert_eq!(size("width"), "400,150");
    assert_eq!(size("inline"), "300,200", "a vertical iframe's inline size is its height");
    assert_eq!(eval(&root, "frames_.any.contentWindow.resized + ' ' + frames_.plain.contentWindow.resized"), "ok NotAllowedError");
    assert_eq!(
        eval(&root, "try { requestResize(); 'ok' } catch (e) { e.name }"),
        "NotAllowedError",
        "a top-level document is not embedded"
    );
}
