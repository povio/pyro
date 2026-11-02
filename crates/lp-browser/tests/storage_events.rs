use lp_browser::{BrowserThread, NavigateOpts, PageId, StaticLoader};
use std::rc::Rc;
use std::time::{Duration, Instant};

fn eval(t: &BrowserThread, page: PageId, source: &str) -> String {
    let s = t.browser.page(page).unwrap().main_frame.main_realm().unwrap().scope().unwrap();
    s.eval(source, None, 1).unwrap_or_else(|e| panic!("{}", e.details(&s).text())).to_string().unwrap()
}
fn settle(t: &mut BrowserThread) {
    assert_eq!(t.run_until_idle(Some(Instant::now() + Duration::from_secs(3))), lp_runtime::RunOutcome::Idle);
}

#[test]
fn storage_changes_reach_other_same_origin_documents_only() {
    // TOS JWT cross-tab login: the magic-link tab stores tokens, the login
    // tab syncs on window `storage` events (login.e2e.ts "loads the original
    // login tab after magic-link authentication in another tab"). HTML Web
    // Storage: a change fires a trusted StorageEvent at every other document
    // sharing the storage area (same origin, same browser context), never at
    // the one that made it; unchanged values and absent keys fire nothing.
    let mut t = crate::browser_thread().unwrap();
    let loader = Rc::new(StaticLoader::default());
    for url in ["http://app.test/a", "http://app.test/b", "http://other.test/c"] {
        loader.insert(url, "text/html", "<p>page</p>", Duration::ZERO);
    }
    t.browser.set_loader(Some(loader));
    let pages: Vec<PageId> = (0..3).map(|_| t.browser.create_page().unwrap()).collect();
    for (page, url) in pages.iter().zip(["http://app.test/a", "http://app.test/b", "http://other.test/c"]) {
        t.browser.navigate(*page, NavigateOpts::address_bar(url)).unwrap();
    }
    settle(&mut t);
    let (a, b, c) = (pages[0], pages[1], pages[2]);
    let listen = "globalThis.log=[];addEventListener('storage',e=>log.push([e.key,e.oldValue,e.newValue,e.url,\
        e.storageArea===localStorage,e.storageArea===sessionStorage,e.isTrusted,e instanceof StorageEvent,e.bubbles,e.cancelable].join(',')));'ok'";
    // Page b never touches storage before the change; it still hears it.
    for page in [a, b, c] {
        assert_eq!(eval(&t, page, listen), "ok");
    }
    assert_eq!(
        eval(
            &t,
            a,
            "localStorage.setItem('k','1');localStorage.setItem('k','1');localStorage.k='2';localStorage.removeItem('k');\
             localStorage.removeItem('k');localStorage.clear();localStorage.setItem('x','y');localStorage.clear();\
             sessionStorage.setItem('s','1');'done'"
        ),
        "done"
    );
    assert_eq!(eval(&t, b, "log.length"), "0", "events are queued, not dispatched inside setItem");
    settle(&mut t);
    let url = "http://app.test/a";
    assert_eq!(
        eval(&t, b, "log.join('|')"),
        [
            format!("k,,1,{url},true,false,true,true,false,false"),
            format!("k,1,2,{url},true,false,true,true,false,false"),
            format!("k,2,,{url},true,false,true,true,false,false"),
            format!("x,,y,{url},true,false,true,true,false,false"),
            format!(",,,{url},true,false,true,true,false,false"),
        ]
        .join("|"),
        "set, overwrite, remove, set, clear (null key); sessionStorage stays in its tab"
    );
    assert_eq!(eval(&t, a, "log.length"), "0", "the changing document gets no event");
    assert_eq!(eval(&t, c, "log.length"), "0", "other origins get no event");
    // The change is visible to the receiver.
    assert_eq!(eval(&t, b, "localStorage.getItem('x')+':'+localStorage.length"), "null:0");
    // Script-constructed events.
    assert_eq!(
        eval(
            &t,
            b,
            "const e=new StorageEvent('storage',{key:'q',newValue:'v',storageArea:localStorage,url:'u'});\
             [e.key,e.oldValue,e.newValue,e.url,e.storageArea===localStorage,e.isTrusted,new StorageEvent('x').key,\
             new StorageEvent('x').storageArea].join(',')"
        ),
        "q,,v,u,true,false,,"
    );
}
