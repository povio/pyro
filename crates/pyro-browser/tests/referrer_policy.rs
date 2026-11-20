//! Referrer Policy through the browser: every source of a policy (the
//! `Referrer-Policy` response header, `<meta name=referrer>`, an element's
//! `referrerpolicy`, `rel=noreferrer`) and every policy's referrer for
//! same-origin, cross-origin and downgrade requests, for subresources
//! (script, image, style sheet), frames and link navigations. The loader
//! records the `Referer` each request would be sent with
//! (`StaticLoader::referers`); redirect chains, fetch and XHR are covered by
//! pyro-net's and pyro-webapi's tests and tests/referrer.rs.
use pyro_browser::{BrowserThread, NavigateOpts, PageId, StaticLoader};
use std::rc::Rc;
use std::time::{Duration, Instant};

const PAGE: &str = "https://site.test/dir/page.html?q=1";
const FULL: Option<&str> = Some(PAGE);
const ORIGIN: Option<&str> = Some("https://site.test/");
const SAME: &str = "https://site.test";
const CROSS: &str = "https://other.test";
const DOWNGRADE: &str = "http://other.test";

/// Each policy with the referrer it gives a same-origin, a cross-origin and
/// a downgrade (https to http) request. `""` is no policy: the default,
/// strict-origin-when-cross-origin.
const POLICIES: [(&str, [Option<&str>; 3]); 9] = [
    ("", [FULL, ORIGIN, None]),
    ("no-referrer", [None, None, None]),
    ("no-referrer-when-downgrade", [FULL, FULL, None]),
    ("same-origin", [FULL, None, None]),
    ("origin", [ORIGIN, ORIGIN, ORIGIN]),
    ("strict-origin", [ORIGIN, ORIGIN, None]),
    ("origin-when-cross-origin", [FULL, ORIGIN, ORIGIN]),
    ("strict-origin-when-cross-origin", [FULL, ORIGIN, None]),
    ("unsafe-url", [FULL, FULL, FULL]),
];

/// PNG signature + IHDR of a 1x1 image.
fn png() -> Vec<u8> {
    let mut b = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
    b.extend_from_slice(&[0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0]);
    b
}

fn eval(thread: &BrowserThread, page: PageId, source: &str) -> String {
    let realm = thread.browser.page(page).unwrap().main_frame.main_realm().unwrap();
    let scope = realm.scope().unwrap();
    scope.eval(source, None, 1).unwrap_or_else(|e| panic!("{}", e.details(&scope).text())).to_string().unwrap()
}

fn settle(thread: &mut BrowserThread) {
    assert_ne!(thread.run_until_idle(Some(Instant::now() + Duration::from_secs(5))), pyro_runtime::RunOutcome::TimedOut);
}

/// A loader with the subresources of every target origin, and a page.
fn setup() -> (BrowserThread, PageId, Rc<StaticLoader>) {
    let mut thread = crate::browser_thread().unwrap();
    let page = thread.browser.create_page().unwrap();
    let loader = Rc::new(StaticLoader::default());
    for origin in [SAME, CROSS, DOWNGRADE] {
        loader.insert(&format!("{origin}/s.js"), "text/javascript", "globalThis.scripts = (globalThis.scripts || 0) + 1;", Duration::ZERO);
        loader.insert(&format!("{origin}/c.css"), "text/css", "p { color: green }", Duration::ZERO);
        loader.insert_bytes(&format!("{origin}/i.png"), "image/png", &png(), Duration::ZERO);
        loader.insert(&format!("{origin}/f.html"), "text/html", "<p>frame</p>", Duration::ZERO);
        loader.insert(&format!("{origin}/n.html"), "text/html", "<p>navigated</p>", Duration::ZERO);
    }
    thread.browser.set_loader(Some(loader.clone()));
    (thread, page, loader)
}

/// Where a case's policy comes from.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Source {
    Header,
    Meta,
    Attribute,
}

/// Loads a page whose script, image, style sheet and iframe request each of
/// the three origins under `policy` delivered by `source`, and checks every
/// request's `Referer`, and each frame's `document.referrer`.
fn check_subresources(source: Source, policy: &str, expected: [Option<&str>; 3]) {
    let (mut thread, page, loader) = setup();
    let attribute = if source == Source::Attribute { format!(" referrerpolicy=\"{policy}\"") } else { String::new() };
    let meta = if source == Source::Meta { format!("<meta name=referrer content=\"{policy}\">") } else { String::new() };
    let mut body = String::new();
    for origin in [SAME, CROSS, DOWNGRADE] {
        body.push_str(&format!(
            "<script src=\"{origin}/s.js\"{attribute}></script><img src=\"{origin}/i.png\"{attribute}>\
             <link rel=stylesheet href=\"{origin}/c.css\"{attribute}><iframe src=\"{origin}/f.html\"{attribute}></iframe>"
        ));
    }
    loader.insert(PAGE, "text/html", &format!("<!doctype html><head>{meta}</head><body>{body}</body>"), Duration::ZERO);
    if source == Source::Header {
        loader.set_referrer_policy(PAGE, policy);
    }
    thread.browser.navigate(page, NavigateOpts::address_bar(PAGE)).unwrap();
    settle(&mut thread);
    assert_eq!(eval(&thread, page, "String(globalThis.scripts)"), "3", "{source:?} {policy}: the scripts ran");
    for (origin, want) in [SAME, CROSS, DOWNGRADE].into_iter().zip(expected) {
        for file in ["s.js", "i.png", "c.css", "f.html"] {
            let url = format!("{origin}/{file}");
            let sent = loader.referer_of(&url).unwrap_or_else(|| panic!("{source:?} {policy}: {url} was not requested"));
            assert_eq!(sent.as_deref(), want, "{source:?} policy {policy:?}: Referer of {url}");
        }
    }
    // A frame's `document.referrer` is what its navigation request was sent.
    for (index, want) in expected.into_iter().enumerate() {
        let frame = eval(&thread, page, &format!("try {{ frames[{index}].document.referrer }} catch (e) {{ 'cross-origin' }}"));
        if index == 0 {
            assert_eq!(frame, want.unwrap_or(""), "{source:?} policy {policy:?}: document.referrer of the same-origin frame");
        }
    }
}

#[test]
fn each_policy_from_the_response_header() {
    for (policy, expected) in POLICIES {
        check_subresources(Source::Header, policy, expected);
    }
}

#[test]
fn each_policy_from_a_meta_element() {
    for (policy, expected) in POLICIES {
        check_subresources(Source::Meta, policy, expected);
    }
}

#[test]
fn each_policy_from_the_referrerpolicy_attribute() {
    for (policy, expected) in POLICIES {
        check_subresources(Source::Attribute, policy, expected);
    }
}

/// The attribute wins over the document's policy, whichever is stricter;
/// its empty state (missing, empty, an unknown or legacy keyword) leaves
/// the document's.
#[test]
fn the_attribute_overrides_the_documents_policy_and_its_empty_state_does_not() {
    let (mut thread, page, loader) = setup();
    loader.insert(
        PAGE,
        "text/html",
        "<!doctype html><meta name=referrer content=no-referrer>\
         <script src=\"https://other.test/s.js\" referrerpolicy=unsafe-url></script>\
         <img src=\"https://other.test/i.png\" referrerpolicy=ORIGIN>\
         <link rel=stylesheet href=\"https://other.test/c.css\" referrerpolicy=bogus>\
         <iframe src=\"https://other.test/f.html\" referrerpolicy=always></iframe>\
         <img src=\"https://site.test/i.png\" referrerpolicy=\"\">",
        Duration::ZERO,
    );
    thread.browser.navigate(page, NavigateOpts::address_bar(PAGE)).unwrap();
    settle(&mut thread);
    assert_eq!(loader.referer_of("https://other.test/s.js"), Some(FULL.map(str::to_owned)));
    assert_eq!(loader.referer_of("https://other.test/i.png"), Some(ORIGIN.map(str::to_owned)));
    for document_policy in ["https://other.test/c.css", "https://other.test/f.html", "https://site.test/i.png"] {
        assert_eq!(loader.referer_of(document_policy), Some(None), "{document_policy}");
    }
    // The reflected IDL attribute is limited to the known keywords.
    assert_eq!(
        eval(&thread, page, "[...document.querySelectorAll('[referrerpolicy]')].map(e => e.referrerPolicy).join()"),
        "unsafe-url,origin,,,"
    );
}

/// A `<meta name=referrer>` sets the document's policy when it is inserted
/// or its `content` changes, wherever it is; an unknown value or the
/// element's removal leaves the policy as it was; legacy keywords map.
#[test]
fn meta_sets_the_policy_when_processed_and_keeps_it_when_removed() {
    let (mut thread, page, loader) = setup();
    loader.set_referrer_policy(PAGE, "unsafe-url");
    loader.insert(PAGE, "text/html", "<!doctype html><body><p>page</p></body>", Duration::ZERO);
    thread.browser.navigate(page, NavigateOpts::address_bar(PAGE)).unwrap();
    settle(&mut thread);
    let mut request = |thread: &mut BrowserThread, step: &str| {
        let url = format!("https://other.test/s.js?{}", loader.referers.borrow().len());
        loader.insert(&url, "text/javascript", "", Duration::ZERO);
        eval(thread, page, &format!("{step}; var s = document.createElement('script'); s.src = '{url}'; document.body.append(s); 0"));
        settle(thread);
        loader.referer_of(&url).unwrap_or_else(|| panic!("{url} was not requested"))
    };
    // The header's policy.
    assert_eq!(request(&mut thread, "0").as_deref(), FULL);
    // A meta in the body counts too.
    let step = "var m = document.createElement('meta'); m.name = 'referrer'; m.content = 'origin'; document.body.append(m)";
    assert_eq!(request(&mut thread, step).as_deref(), ORIGIN);
    // Unknown and empty values change nothing.
    assert_eq!(request(&mut thread, "m.content = 'bogus'").as_deref(), ORIGIN);
    assert_eq!(request(&mut thread, "m.content = ''").as_deref(), ORIGIN);
    // A changed content is processed again; legacy keywords map.
    assert_eq!(request(&mut thread, "m.content = 'never'").as_deref(), None);
    assert_eq!(request(&mut thread, "m.content = 'always'").as_deref(), FULL);
    assert_eq!(request(&mut thread, "m.content = 'Same-Origin'").as_deref(), None);
    // Removal does not bring the header's policy back.
    assert_eq!(request(&mut thread, "m.remove()").as_deref(), None);
    // The most recently processed meta wins, not the first in the tree.
    let step = "var a = document.createElement('meta'); a.name = 'referrer'; a.content = 'origin'; document.head.append(a); \
                var b = document.createElement('meta'); b.name = 'referrer'; b.content = 'no-referrer'; document.head.prepend(b)";
    assert_eq!(request(&mut thread, step).as_deref(), None);
    assert_eq!(request(&mut thread, "a.content = 'unsafe-url'").as_deref(), FULL);
    // A meta that is not in the document does nothing.
    let step = "var d = document.createElement('meta'); d.name = 'referrer'; d.content = 'no-referrer'";
    assert_eq!(request(&mut thread, step).as_deref(), FULL);
}

/// A followed link: the document's policy, the link's `referrerpolicy`,
/// `rel=noreferrer`, for HTML `a`, `area` and SVG `a`, and the navigated
/// document's `document.referrer`.
#[test]
fn link_navigations_use_the_links_policy_and_noreferrer() {
    let cases: [(&str, &str, Option<&str>); 9] = [
        // (document policy, link markup, Referer of the cross-origin navigation)
        ("", "<a id=l href=\"https://other.test/n.html\">x</a>", ORIGIN),
        ("unsafe-url", "<a id=l href=\"https://other.test/n.html\">x</a>", FULL),
        ("unsafe-url", "<a id=l href=\"https://other.test/n.html\" referrerpolicy=no-referrer>x</a>", None),
        ("no-referrer", "<a id=l href=\"https://other.test/n.html\" referrerpolicy=unsafe-url>x</a>", FULL),
        ("unsafe-url", "<a id=l href=\"https://other.test/n.html\" rel=\"nofollow NOREFERRER\">x</a>", None),
        ("", "<a id=l href=\"https://other.test/n.html\" rel=noreferrer referrerpolicy=unsafe-url>x</a>", None),
        ("", "<map><area id=l href=\"https://other.test/n.html\" referrerpolicy=unsafe-url></map>", FULL),
        ("", "<svg><a id=l href=\"https://other.test/n.html\" referrerpolicy=unsafe-url><text>x</text></a></svg>", FULL),
        ("unsafe-url", "<svg><a id=l href=\"https://other.test/n.html\"><text>x</text></a></svg>", FULL),
    ];
    for (document_policy, link, want) in cases {
        let (mut thread, page, loader) = setup();
        loader.insert(PAGE, "text/html", &format!("<!doctype html><body>{link}</body>"), Duration::ZERO);
        if !document_policy.is_empty() {
            loader.set_referrer_policy(PAGE, document_policy);
        }
        thread.browser.navigate(page, NavigateOpts::address_bar(PAGE)).unwrap();
        settle(&mut thread);
        eval(&thread, page, "document.getElementById('l').dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true })); 0");
        settle(&mut thread);
        assert_eq!(eval(&thread, page, "location.href"), "https://other.test/n.html", "{link}");
        assert_eq!(loader.referer_of("https://other.test/n.html"), Some(want.map(str::to_owned)), "{document_policy:?} {link}");
        assert_eq!(eval(&thread, page, "document.referrer"), want.unwrap_or(""), "{document_policy:?} {link}");
    }
}

/// A link that targets a frame is sent with the policy of the document that
/// holds the link, not of the frame's current document; the frame's
/// `document.referrer` follows.
#[test]
fn a_link_into_a_frame_uses_the_navigating_documents_policy() {
    for (policy, same, cross) in [("same-origin", FULL, None), ("origin", ORIGIN, ORIGIN), ("no-referrer", None, None)] {
        let (mut thread, page, loader) = setup();
        loader.set_referrer_policy(PAGE, policy);
        loader.insert(
            PAGE,
            "text/html",
            "<!doctype html><body><iframe name=f></iframe>\
             <a id=same target=f href=\"https://site.test/n.html\">same</a>\
             <a id=cross target=f href=\"https://other.test/n.html\">cross</a></body>",
            Duration::ZERO,
        );
        thread.browser.navigate(page, NavigateOpts::address_bar(PAGE)).unwrap();
        settle(&mut thread);
        eval(&thread, page, "document.getElementById('same').click(); 0");
        settle(&mut thread);
        assert_eq!(loader.referer_of("https://site.test/n.html"), Some(same.map(str::to_owned)), "{policy}");
        assert_eq!(eval(&thread, page, "frames[0].document.referrer"), same.unwrap_or(""), "{policy}");
        eval(&thread, page, "document.getElementById('cross').click(); 0");
        settle(&mut thread);
        assert_eq!(loader.referer_of("https://other.test/n.html"), Some(cross.map(str::to_owned)), "{policy}");
    }
}

/// An `<iframe srcdoc>` document starts with its container document's
/// policy (its policy container is a clone), and a `<meta name=referrer>`
/// of its own replaces it.
#[test]
fn a_srcdoc_document_inherits_its_containers_policy() {
    let (mut thread, page, loader) = setup();
    loader.set_referrer_policy(PAGE, "same-origin");
    loader.insert(
        PAGE,
        "text/html",
        "<!doctype html><body>\
         <iframe srcdoc=\"<img src='https://other.test/i.png?inherited'><img src='https://site.test/i.png?inherited'>\"></iframe>\
         <iframe srcdoc=\"<meta name=referrer content=unsafe-url><img src='https://other.test/i.png?own'>\"></iframe></body>",
        Duration::ZERO,
    );
    for url in ["https://other.test/i.png?inherited", "https://site.test/i.png?inherited", "https://other.test/i.png?own"] {
        loader.insert_bytes(url, "image/png", &png(), Duration::ZERO);
    }
    thread.browser.navigate(page, NavigateOpts::address_bar(PAGE)).unwrap();
    settle(&mut thread);
    assert_eq!(loader.referer_of("https://other.test/i.png?inherited"), Some(None));
    assert_eq!(loader.referer_of("https://site.test/i.png?inherited"), Some(FULL.map(str::to_owned)));
    assert_eq!(loader.referer_of("https://other.test/i.png?own"), Some(FULL.map(str::to_owned)));
}

/// A style sheet's requests are sent under the sheet's own `Referrer-Policy` when it was served
/// with one (as Chrome sends them): its `@import`s carry it from the sheet that imports them, and
/// the renderer gets it for its `url()`s on the sheet's link in the render snapshot (an engine
/// attribute), never by looking its URL up (security review cycles 1-4, MEDIUM 6 / S1).
#[test]
fn a_sheets_requests_follow_its_referrer_policy() {
    let (mut thread, page, loader) = setup();
    let sheet = format!("{SAME}/strict.css");
    loader.insert(&sheet, "text/css", &format!("@import \"{SAME}/imported.css\"; p {{ color: red }}"), Duration::ZERO);
    loader.set_referrer_policy(&sheet, "no-referrer");
    loader.insert(&format!("{SAME}/imported.css"), "text/css", "p { margin: 0 }", Duration::ZERO);
    loader.insert(PAGE, "text/html", &format!("<!doctype html><link id=l rel=stylesheet href=\"{sheet}\">"), Duration::ZERO);
    loader.set_referrer_policy(PAGE, "unsafe-url");
    thread.browser.navigate(page, NavigateOpts::address_bar(PAGE)).unwrap();
    settle(&mut thread);
    assert_eq!(loader.referer_of(&format!("{SAME}/imported.css")), Some(None), "the import");
    let page_dom = thread.browser.page(page).unwrap().main_frame.page_dom.clone().unwrap();
    let html = page_dom.render_snapshot(PAGE, None).html;
    assert!(html.contains("data-pyro-sheet-referrer-policy=\"no-referrer\""), "{html}");
}

/// A sheet's `Referrer-Policy` is its current response's: once the link loads a response without
/// one (or fails), the snapshot gives its sheet none; a page's own attribute of that name is never
/// written (security review cycles 2-4, 2b / S1).
#[test]
fn a_sheets_referrer_policy_goes_with_its_response() {
    let (mut thread, page, loader) = setup();
    let sheet = format!("{SAME}/strict.css");
    loader.insert(&sheet, "text/css", "p { color: red }", Duration::ZERO);
    loader.set_referrer_policy(&sheet, "no-referrer");
    loader.insert(&format!("{SAME}/plain.css"), "text/css", "p { color: blue }", Duration::ZERO);
    loader.insert(
        PAGE,
        "text/html",
        &format!("<!doctype html><link id=l rel=stylesheet href=\"{sheet}\"><link id=f rel=stylesheet href=\"{SAME}/plain.css\" data-pyro-sheet-referrer-policy=unsafe-url>"),
        Duration::ZERO,
    );
    thread.browser.navigate(page, NavigateOpts::address_bar(PAGE)).unwrap();
    settle(&mut thread);
    let snapshot = |thread: &BrowserThread| {
        let page_dom = thread.browser.page(page).unwrap().main_frame.page_dom.clone().unwrap();
        page_dom.render_snapshot(PAGE, None).html
    };
    let html = snapshot(&thread);
    assert_eq!(html.matches("data-pyro-sheet-referrer-policy").count(), 1, "{html}");
    assert!(!html.contains("unsafe-url"), "the page's own attribute is not written: {html}");
    eval(&thread, page, &format!("document.getElementById('l').href = '{SAME}/plain.css'; 1"));
    settle(&mut thread);
    let html = snapshot(&thread);
    assert!(!html.contains("data-pyro-sheet-referrer-policy"), "{html}");
}
