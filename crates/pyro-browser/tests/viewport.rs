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
