use pyro_browser::{BrowserThread, InitScript, NavigateOpts, PageId, StaticLoader};
use pyro_dom::media::{ColorScheme, Viewport};
use std::rc::Rc;
use std::time::{Duration, Instant};

const SNAPSHOT: &str = "[innerWidth,innerHeight,devicePixelRatio,screen.width,screen.height,matchMedia('(prefers-color-scheme: dark)').matches].join('|')";
fn eval(t: &BrowserThread, page: PageId, source: &str) -> String {
    let s = t.browser.page(page).unwrap().main_frame.main_realm().unwrap().scope().unwrap();
    s.eval(source, None, 1).unwrap_or_else(|e| panic!("{}", e.details(&s).text())).to_string().unwrap()
}
fn settle(t: &mut BrowserThread) {
    assert_eq!(t.run_until_idle(Some(Instant::now() + Duration::from_secs(3))), pyro_runtime::RunOutcome::Idle);
}

#[test]
fn viewport_persists_before_init_scripts_and_isolates_sibling_pages() {
    let mut t = crate::browser_thread().unwrap();
    let a = t.browser.create_page().unwrap();
    let b = t.browser.create_page().unwrap();
    let original = eval(&t, b, SNAPSHOT);
    let world = t.browser.isolated_world(a, "existing").unwrap();
    let viewport = Viewport {
        width: 700,
        height: 500,
        device_scale_factor: 2.0,
        screen_width: Some(900),
        screen_height: Some(800),
        color_scheme: ColorScheme::Dark,
        ..Default::default()
    };
    t.browser.set_viewport(a, viewport.clone()).unwrap();
    assert_eq!(t.browser.page(a).unwrap().viewport(), &viewport);
    assert_eq!(eval(&t, a, SNAPSHOT), "700|500|2|900|800|true");
    assert_eq!(world.scope().unwrap().eval(SNAPSHOT, None, 1).unwrap().to_string().unwrap(), "700|500|2|900|800|true");
    assert_eq!(eval(&t, b, SNAPSHOT), original);
    for world in [None, Some("new".into())] {
        t.browser.add_init_script(
            a,
            InitScript {
                id: format!("{world:?}"),
                world,
                source: format!(
                    "globalThis.atInit={SNAPSHOT}; globalThis.resizes=0; addEventListener('resize',()=>resizes++)"
                ),
            },
        );
    }
    let loader = Rc::new(StaticLoader::default());
    loader.insert(
        "http://view.test/",
        "text/html",
        "<style>@media (max-width:800px) { p {display:none} }</style><p>viewport</p>",
        Duration::ZERO,
    );
    t.browser.set_loader(Some(loader));
    t.browser.navigate(a, NavigateOpts::address_bar("http://view.test/")).unwrap();
    settle(&mut t);
    assert_eq!(eval(&t, a, "atInit"), "700|500|2|900|800|true");
    assert_eq!(eval(&t, a, "resizes"), "0", "old document resize reached replacement");
    assert_eq!(eval(&t, a, "getComputedStyle(document.querySelector('p')).display"), "none");
    let new = t.browser.isolated_world(a, "new").unwrap();
    assert_eq!(new.scope().unwrap().eval("atInit", None, 1).unwrap().to_string().unwrap(), "700|500|2|900|800|true");
    t.browser.set_viewport(a, Viewport::default()).unwrap();
    settle(&mut t);
    assert_eq!(eval(&t, a, SNAPSHOT), original);
    assert_eq!(eval(&t, a, "getComputedStyle(document.querySelector('p')).display"), "block");
    assert_eq!(eval(&t, a, "resizes"), "1");
    assert_eq!(eval(&t, b, SNAPSHOT), original);
}

#[test]
fn resize_delivery_is_coalesced_trusted_and_dropped_on_navigation_or_close() {
    let mut t = crate::browser_thread().unwrap();
    let page = t.browser.create_page().unwrap();
    let world = t.browser.isolated_world(page, "automation").unwrap();
    eval(&t, page, "globalThis.events=[];addEventListener('resize', e=>{events.push([innerWidth,e.isTrusted,e.bubbles,e.cancelable,e.target===window].join(':'));document.body.textContent='resized'})");
    world.scope().unwrap().eval("globalThis.events=0;addEventListener('resize',()=>events++)", None, 1).unwrap();
    t.browser.set_viewport(page, Viewport::new(800, 600)).unwrap();
    t.browser.set_viewport(page, Viewport::new(900, 700)).unwrap();
    assert_eq!(eval(&t, page, "events.length"), "0", "resize ran inside setter");
    settle(&mut t);
    assert_eq!(eval(&t, page, "events.join('|')"), "900:true:false:false:true");
    assert_eq!(world.scope().unwrap().eval("events", None, 1).unwrap().to_number().unwrap(), 1.0);
    // A transient change restored before the safe point has no observable resize.
    t.browser.set_viewport(page, Viewport::new(1000, 700)).unwrap();
    t.browser.set_viewport(page, Viewport::new(900, 700)).unwrap();
    settle(&mut t);
    assert_eq!(eval(&t, page, "events.length"), "1");
    let mut dpr_only = t.browser.page(page).unwrap().viewport().clone();
    dpr_only.device_scale_factor = 3.0;
    t.browser.set_viewport(page, dpr_only).unwrap();
    settle(&mut t);
    assert_eq!(eval(&t, page, "events.length"), "1");
    for dpr in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        let invalid = Viewport { width: 1, device_scale_factor: dpr, ..Default::default() };
        assert!(t.browser.set_viewport(page, invalid).is_err());
        assert_eq!(eval(&t, page, "innerWidth+':'+devicePixelRatio"), "900:3");
    }
    let weak = Rc::downgrade(t.browser.page(page).unwrap().main_frame.page_dom.as_ref().unwrap());
    t.browser.set_viewport(page, Viewport::new(1200, 800)).unwrap();
    t.browser.navigate(page, NavigateOpts::address_bar("about:blank")).unwrap();
    eval(&t, page, "globalThis.events=0;addEventListener('resize',()=>events++)");
    settle(&mut t);
    assert_eq!(eval(&t, page, "events"), "0");
    drop(world);
    t.browser.engine.collect_garbage_now();
    t.browser.engine.drain_finalized();
    assert!(weak.upgrade().is_none(), "resize task retained old document");
    t.browser.set_viewport(page, Viewport::new(1300, 800)).unwrap();
    assert!(t.browser.close_page(page));
    settle(&mut t);
    assert!(t.browser.set_viewport(page, Viewport::default()).is_err());
}

#[test]
fn media_change_releases_parser_wait_without_waiting_for_stylesheet_response() {
    let mut t = crate::browser_thread().unwrap();
    let page = t.browser.create_page().unwrap();
    let loader = Rc::new(StaticLoader::default());
    loader.insert(
        "http://view.test/",
        "text/html",
        "<link rel=stylesheet media='(min-width:1000px)' href='/slow.css'><script>globalThis.ran=true</script>",
        Duration::ZERO,
    );
    loader.insert("http://view.test/slow.css", "text/css", "p {display:none}", Duration::from_secs(1));
    t.browser.set_loader(Some(loader));
    t.browser.navigate(page, NavigateOpts::address_bar("http://view.test/")).unwrap();
    assert_eq!(
        t.run_until(Some(Instant::now() + Duration::from_millis(500)), |b| b.page(page).unwrap().stylesheet_usage().0
            == 1),
        pyro_runtime::RunOutcome::Done
    );
    assert_eq!(eval(&t, page, "typeof ran"), "undefined");
    t.browser.set_viewport(page, Viewport::new(600, 500)).unwrap();
    assert_eq!(
        t.run_until(Some(Instant::now() + Duration::from_millis(500)), |b| {
            b.page(page)
                .unwrap()
                .main_frame
                .main_realm()
                .unwrap()
                .scope()
                .unwrap()
                .eval("globalThis.ran===true", None, 1)
                .unwrap()
                .to_bool()
        }),
        pyro_runtime::RunOutcome::Done
    );
    assert_eq!(
        t.browser.page(page).unwrap().stylesheet_usage().0,
        1,
        "test waited for response instead of media change"
    );
}

#[test]
fn visual_viewport_matches_the_layout_viewport() {
    // react-aria bounds popover max-height by visualViewport.offsetTop +
    // height; without the object every TOS popover got max-height: 0.
    // Chromium without pinch zoom: the layout viewport size, offsets 0,
    // page offsets = scroll position, scale 1, one object per window.
    let mut t = crate::browser_thread().unwrap();
    let page = t.browser.create_page().unwrap();
    t.browser.set_viewport(page, Viewport::new(1440, 900)).unwrap();
    settle(&mut t);
    assert_eq!(
        eval(
            &t,
            page,
            "const v = visualViewport; [v.width, v.height, v.offsetLeft, v.offsetTop, v.pageLeft, v.pageTop, v.scale, v === visualViewport, v instanceof VisualViewport, typeof v.addEventListener].join('|')"
        ),
        "1440|900|0|0|0|0|1|true|true|function"
    );
}

#[test]
fn visual_viewport_and_screen_orientation_are_event_targets() {
    // react-aria >= 3.53 tracks the virtual keyboard at module load:
    // `addEventListener` on window.visualViewport ('resize') and
    // window.screen.orientation ('change'). screen.orientation was a plain
    // object, so the TOS app shell threw and never mounted. Both are
    // EventTargets (VisualViewport, ScreenOrientation): listeners, handlers
    // and dispatch, trusted resize/change from the embedder, per realm.
    let mut t = crate::browser_thread().unwrap();
    let page = t.browser.create_page().unwrap();
    t.browser.set_viewport(page, Viewport::new(1000, 700)).unwrap();
    settle(&mut t);
    let world = t.browser.isolated_world(page, "automation").unwrap();
    // The bundle's helper, as shipped.
    let react_aria = "function cv(e,t,n,r){if(n==null||e==null)return()=>{};let i=Array.isArray(e)?e:[e];\
        for(let e of i)e.addEventListener(t,n,r);return()=>{for(let e of i)e.removeEventListener(t,n,r)}}\
        globalThis.log=[];cv(window.visualViewport,'resize',e=>log.push('vv:'+e.isTrusted+':'+visualViewport.width));\
        cv(window.screen.orientation,'change',e=>log.push('o:'+e.isTrusted+':'+screen.orientation.type+':'+screen.orientation.angle));\
        cv(window,'focus',()=>{},{capture:true,passive:true});'mounted'";
    assert_eq!(eval(&t, page, react_aria), "mounted");
    assert_eq!(
        eval(
            &t,
            page,
            "const v=visualViewport,o=screen.orientation;[v instanceof EventTarget,o instanceof EventTarget,o instanceof ScreenOrientation,\
             o===screen.orientation,o.type,o.angle,typeof o.lock,typeof o.unlock].join('|')"
        ),
        "true|true|true|true|landscape-primary|0|function|function"
    );
    // Script dispatch, handlers and removal.
    assert_eq!(
        eval(
            &t,
            page,
            "const seen=[];const f=e=>seen.push(e.type);visualViewport.addEventListener('scroll',f);\
             visualViewport.onresize=e=>seen.push('on'+e.type);screen.orientation.onchange=e=>seen.push('on'+e.type);\
             visualViewport.dispatchEvent(new Event('scroll'));visualViewport.dispatchEvent(new Event('resize'));\
             screen.orientation.dispatchEvent(new Event('change'));visualViewport.removeEventListener('scroll',f);\
             visualViewport.dispatchEvent(new Event('scroll'));visualViewport.onresize=null;screen.orientation.onchange=null;\
             log.length=0;seen.join(',')"
        ),
        "scroll,onresize,onchange"
    );
    world
        .scope()
        .unwrap()
        .eval(
            "globalThis.n=0;visualViewport.addEventListener('resize',()=>n++);screen.orientation.addEventListener('change',()=>n+=10)",
            None,
            1,
        )
        .unwrap();
    // A resize reaches visualViewport (trusted, after window; Chromium:
    // `vv:true:600`) in every realm. The screen size is emulated, so the
    // orientation does not change with it.
    let sized = |w: u32, h: u32| Viewport { screen_width: Some(1000), screen_height: Some(700), ..Viewport::new(w, h) };
    t.browser.set_viewport(page, sized(600, 900)).unwrap();
    settle(&mut t);
    assert_eq!(eval(&t, page, "log.join('|')"), "vv:true:600");
    assert_eq!(world.scope().unwrap().eval("n", None, 1).unwrap().to_number().unwrap(), 1.0);
    // An emulated rotation reaches screen.orientation, without a resize.
    let rotated = Viewport {
        orientation: Some(pyro_dom::media::ScreenOrientation {
            kind: pyro_dom::media::OrientationType::PortraitPrimary,
            angle: 90,
        }),
        ..sized(600, 900)
    };
    t.browser.set_viewport(page, rotated.clone()).unwrap();
    settle(&mut t);
    assert_eq!(eval(&t, page, "log.join('|')"), "vv:true:600|o:true:portrait-primary:90");
    assert_eq!(world.scope().unwrap().eval("n", None, 1).unwrap().to_number().unwrap(), 11.0);
    // No change, no events.
    t.browser.set_viewport(page, rotated).unwrap();
    settle(&mut t);
    assert_eq!(eval(&t, page, "log.length"), "2");
    // lock() resolves without rotating, unlock() is a no-op (headless Chromium).
    assert_eq!(
        eval(&t, page, "globalThis.locked='pending';screen.orientation.lock('landscape').then(()=>locked='ok',e=>locked=e.name);screen.orientation.unlock();'x'"),
        "x"
    );
    settle(&mut t);
    assert_eq!(eval(&t, page, "locked+':'+screen.orientation.type"), "ok:portrait-primary");
}

#[test]
fn resize_layout_reads_include_new_unstyled_nodes() {
    let mut t = crate::browser_thread().unwrap();
    let page = t.browser.create_page().unwrap();
    let loader = Rc::new(StaticLoader::default());
    loader.insert(
        "http://resize-layout.test/",
        "text/html",
        "<style>body{width:90vw}.new-node{width:20vw}</style><body>resize</body>",
        Duration::ZERO,
    );
    t.browser.set_loader(Some(loader));
    t.browser.navigate(page, NavigateOpts::address_bar("http://resize-layout.test/")).unwrap();
    settle(&mut t);
    eval(&t, page, "document.body.offsetWidth");
    eval(&t, page, r#"
        globalThis.widths = [];
        addEventListener('resize', () => {
            const node = document.createElement('div');
            node.className = 'new-node';
            node.style.height = '20px';
            document.body.append(node);
            widths.push(document.body.offsetWidth);
        });
    "#);
    for width in 700..720 {
        t.browser.set_viewport(page, Viewport::new(width, 600)).unwrap();
        settle(&mut t);
    }
    let expected: Vec<_> = (700..720).map(|width| ((f64::from(width) * 0.9).round() as u32).to_string()).collect();
    assert_eq!(eval(&t, page, "widths.join(',')"), expected.join(","));
}

#[test]
fn viewport_restyles_siblings_and_moved_children_of_unstyled_nodes() {
    let mut t = crate::browser_thread().unwrap();
    let page = t.browser.create_page().unwrap();
    let loader = Rc::new(StaticLoader::default());
    loader.insert(
        "http://resize-move.test/",
        "text/html",
        "<style>body{width:90vw}.box{width:20vw}.child{display:block;width:10vw}</style><body><div id=existing class=box><span id=moved class=child></span></div></body>",
        Duration::ZERO,
    );
    t.browser.set_loader(Some(loader));
    t.browser.navigate(page, NavigateOpts::address_bar("http://resize-move.test/")).unwrap();
    settle(&mut t);
    eval(&t, page, "document.body.offsetWidth");
    eval(&t, page, r#"
        globalThis.widths = [];
        addEventListener('resize', () => {
            const node = document.createElement('div');
            node.className = 'box';
            node.style.height = '20px';
            const child = document.createElement('span');
            child.className = 'child';
            node.append(child);
            document.body.prepend(node);
            const moved = document.getElementById('moved');
            node.append(moved);
            widths.push([document.body.offsetWidth, node.offsetWidth,
                document.getElementById('existing').offsetWidth, moved.offsetWidth, child.offsetWidth].join(':'));
        });
    "#);
    let mut expected = Vec::new();
    for width in 700..720 {
        t.browser.set_viewport(page, Viewport::new(width, 600)).unwrap();
        settle(&mut t);
        let body = (f64::from(width) * 0.9).round() as u32;
        let outer = (f64::from(width) * 0.2).round() as u32;
        let inner = (f64::from(width) * 0.1).round() as u32;
        expected.push(format!("{body}:{outer}:{outer}:{inner}:{inner}"));
    }
    assert_eq!(eval(&t, page, "widths.join('|')"), expected.join("|"));
}

/// Playwright's `devices['iPhone 13']`: a 390x664 viewport at DPR 3 on a
/// 390x844 screen, `isMobile`.
fn iphone_13() -> Viewport {
    Viewport { device_scale_factor: 3.0, screen_width: Some(390), screen_height: Some(844), mobile: true, ..Viewport::new(390, 664) }
}

const PHONE: &str = "[innerWidth, innerHeight, document.documentElement.clientWidth, document.documentElement.clientHeight, \
    visualViewport.width, visualViewport.height, visualViewport.scale, matchMedia('(max-width: 600px)').matches, \
    outerWidth, screen.width, document.body.getBoundingClientRect().width].join('|')";

/// Pages at `http://m.test/<name>`: a paragraph, after the viewport `<meta>`
/// of `content` (none for `None`), then `extra`.
fn mobile_site(pages: &[(&str, Option<&str>, &str)]) -> Rc<StaticLoader> {
    let loader = Rc::new(StaticLoader::default());
    for (name, content, extra) in pages {
        let meta = content.map(|c| format!("<meta name=viewport content='{c}'>")).unwrap_or_default();
        let html = format!("<!doctype html><html><head>{meta}</head><body style='margin:0'><p style='margin:0'>x</p>{extra}</body></html>");
        loader.insert(&format!("http://m.test/{name}"), "text/html", &html, Duration::ZERO);
    }
    loader
}

fn load(t: &mut BrowserThread, page: PageId, name: &str) {
    t.browser.navigate(page, NavigateOpts::address_bar(&format!("http://m.test/{name}"))).unwrap();
    settle(t);
}

#[test]
fn mobile_viewport_follows_the_meta_viewport() {
    // Expected: Chrome 147 (Playwright 1.59.1's Chromium) with
    // devices['iPhone 13'], the same pages.
    let mut t = crate::browser_thread().unwrap();
    let page = t.browser.create_page().unwrap();
    t.browser.set_loader(Some(mobile_site(&[
        ("dw", Some("width=device-width, initial-scale=1"), ""),
        ("none", None, ""),
        ("w600", Some("width=600"), ""),
        ("is2", Some("initial-scale=2"), ""),
        ("dw-is2", Some("width=device-width, initial-scale=2"), ""),
        ("wide", Some("width=device-width, initial-scale=1"), "<div style='width:1200px;height:10px'></div>"),
        ("wide-none", None, "<div style='width:1500px;height:10px'></div>"),
        (
            "wide-fixed",
            Some("width=device-width, initial-scale=1, user-scalable=no"),
            "<div style='width:1200px;height:10px'></div>",
        ),
    ])));
    t.browser.set_viewport(page, iphone_13()).unwrap();
    for (name, expected) in [
        ("dw", "390|664|390|664|390|664|1|true|390|390|390"),
        // No tag: a 980 px layout viewport, zoomed out to the device's width.
        ("none", "980|1669|980|1668|980|1668.5128173828125|0.3979591727256775|false|390|390|980"),
        ("w600", "600|1022|600|1021|600|1021.5384521484375|0.6499999761581421|true|390|390|600"),
        ("is2", "195|332|195|332|195|332|2|true|390|390|195"),
        ("dw-is2", "390|664|390|664|195|332|2|true|390|390|390"),
        // Content wider than the layout viewport widens the window, not the
        // layout viewport; without an initial scale the page zooms out to it.
        ("wide", "1200|2044|390|664|390|664|1|true|390|390|390"),
        ("wide-none", "1500|2554|980|1668|1500|2553.84619140625|0.25999999046325684|false|390|390|980"),
        // The tag's limits hold: `user-scalable=no` locks the scale at 1.
        ("wide-fixed", "390|664|390|664|390|664|1|true|390|390|390"),
    ] {
        load(&mut t, page, name);
        assert_eq!(eval(&t, page, PHONE), expected, "{name}");
    }
    assert_eq!(
        eval(&t, page, "[document.documentElement.scrollWidth, document.documentElement.scrollHeight].join()"),
        "1200,664"
    );
    // Desktop (mobile: false) ignores the tag, as before.
    t.browser.set_viewport(page, Viewport { mobile: false, ..iphone_13() }).unwrap();
    load(&mut t, page, "w600");
    assert_eq!(eval(&t, page, PHONE), "390|664|390|664|390|664|1|true|390|390|390");
    load(&mut t, page, "none");
    assert_eq!(eval(&t, page, PHONE), "390|664|390|664|390|664|1|true|390|390|390");
}

#[test]
fn mobile_meta_viewport_changes_by_script_relayout_and_resize() {
    let mut t = crate::browser_thread().unwrap();
    let page = t.browser.create_page().unwrap();
    let listeners = "<script>globalThis.ev=[];addEventListener('resize',()=>ev.push('w:'+innerWidth+'x'+innerHeight));\
        visualViewport.addEventListener('resize',()=>ev.push('vv:'+visualViewport.width+'@'+visualViewport.scale.toFixed(3)));\
        matchMedia('(max-width: 600px)').addEventListener('change',e=>ev.push('mq:'+e.matches))</script>";
    let loader = mobile_site(&[("none", None, listeners)]);
    let head = format!("<!doctype html><html><head>{listeners}<meta name=viewport content='width=device-width'></head><body>x</body></html>");
    loader.insert("http://m.test/head", "text/html", &head, Duration::ZERO);
    t.browser.set_loader(Some(loader));
    t.browser.set_viewport(page, iphone_13()).unwrap();
    // A tag the parser meets before the first layout fires nothing (Chrome: []).
    load(&mut t, page, "head");
    assert_eq!(eval(&t, page, "[innerWidth, ev.join()].join('|')"), "390|");
    load(&mut t, page, "none");
    assert_eq!(eval(&t, page, "[innerWidth, ev.join()].join('|')"), "980|");
    // Added by script: the next read lays out again (Chrome: the same values
    // at once), and resize, visual viewport resize and the media query list
    // change follow, in Chrome's order.
    let snap = "[innerWidth, innerHeight, document.documentElement.clientWidth, visualViewport.width, visualViewport.scale, matchMedia('(max-width: 600px)').matches].join('|')";
    let added = format!("const m=document.createElement('meta');m.name='viewport';m.id='m';m.content='width=device-width, initial-scale=1';document.head.append(m);{snap}");
    assert_eq!(eval(&t, page, &added), "390|664|390|390|1|true");
    settle(&mut t);
    assert_eq!(eval(&t, page, "ev.join()"), "w:390x664,vv:390@1.000,mq:true");
    // A changed content (Chrome: 600|1022|600|600|0.65|true, then resize and
    // visual viewport resize).
    let changed = format!("m.setAttribute('content','width=600');{snap}");
    assert_eq!(eval(&t, page, &changed), "600|1022|600|600|0.6499999761581421|true");
    settle(&mut t);
    assert_eq!(eval(&t, page, "ev.join()"), "w:390x664,vv:390@1.000,mq:true,w:600x1022,vv:600@0.650");
    // Removing the tag, renaming it, a tag without content or in a shadow
    // tree change nothing (as in Chrome).
    let ignored = "m.remove();const n=document.createElement('meta');n.name='viewport';document.head.append(n);\
        const host=document.body.appendChild(document.createElement('div'));const s=document.createElement('meta');\
        s.name='viewport';s.content='width=300';host.attachShadow({mode:'open'}).append(s);";
    assert_eq!(eval(&t, page, &format!("{ignored}{snap}")), "600|1022|600|600|0.6499999761581421|true");
    // The latest tag processed wins, wherever it is in the tree.
    let prepended = format!("const p=document.createElement('meta');p.name='viewport';p.content='width=700';document.head.prepend(p);{snap}");
    assert_eq!(eval(&t, page, &prepended), "700|1192|700|700|0.5571428537368774|false");
    settle(&mut t);
    assert_eq!(eval(&t, page, "ev.slice(5).join()"), "w:700x1192,vv:700@0.557,mq:false");
    // Leaving mobile emulation lays the page out at the device's width.
    t.browser.set_viewport(page, Viewport { mobile: false, ..iphone_13() }).unwrap();
    settle(&mut t);
    assert_eq!(
        eval(&t, page, &format!("{snap}+'|'+ev.slice(8).join()")),
        "390|664|390|390|1|true|w:390x664,vv:390@1.000,mq:true"
    );
}
