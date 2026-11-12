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
            body: b"<iframe></iframe><script>globalThis.late=true</script>".to_vec(),
            content_disposition: None,
            header_csp: false,
            refresh: None,
            referrer_policy: None,
            timing: None,
            partial: None,
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
fn cross_document_wrappers_survive_gc_and_throw_after_the_child_navigates() {
    let (mut t, page, root) = cross_document_page();
    eval(&root, "nodes[0].mark=7;nodes[0].addEventListener('x',()=>globalThis.fired=true)");
    eval(&root, "globalThis.first=nodes[0];globalThis.stale=nodes;globalThis.staleDoc=d;delete globalThis.nodes");
    t.browser.engine.collect_garbage_now();
    assert_eq!(eval(&root, "stale[0].mark+':'+(stale[0]===first)"), "7:true");
    let child = t.browser.frames(page).into_iter().find(|f| f.parent.is_some()).unwrap();
    t.browser.navigate_frame(page, &child.id, NavigateOpts::address_bar("about:blank")).unwrap();
    settle(&mut t);
    let destroyed = "Execution context was destroyed";
    for expr in ["stale[0]", "stale.length", "staleDoc.querySelector('span')", "first.dataset.x"] {
        let message = eval(&root, &format!("try{{{expr};'no throw'}}catch(e){{String(e.message||e)}}"));
        assert!(message.contains(destroyed), "{expr}: {message}");
    }
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
