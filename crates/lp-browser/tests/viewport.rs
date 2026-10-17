use lp_browser::{BrowserThread, InitScript, NavigateOpts, PageId, StaticLoader};
use lp_dom::media::{ColorScheme, Viewport};
use std::rc::Rc;
use std::time::{Duration, Instant};

const SNAPSHOT: &str = "[innerWidth,innerHeight,devicePixelRatio,screen.width,screen.height,matchMedia('(prefers-color-scheme: dark)').matches].join('|')";
fn eval(t: &BrowserThread, page: PageId, source: &str) -> String {
    let s = t.browser.page(page).unwrap().main_frame.main_realm().unwrap().scope().unwrap();
    s.eval(source, None, 1).unwrap_or_else(|e| panic!("{}", e.details(&s).text())).to_string().unwrap()
}
fn settle(t: &mut BrowserThread) {
    assert_eq!(t.run_until_idle(Some(Instant::now() + Duration::from_secs(3))), lp_runtime::RunOutcome::Idle);
}

#[test]
fn viewport_persists_before_init_scripts_and_isolates_sibling_pages() {
    let mut t = BrowserThread::new().unwrap();
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
    let mut t = BrowserThread::new().unwrap();
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
    let mut t = BrowserThread::new().unwrap();
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
        lp_runtime::RunOutcome::Done
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
        lp_runtime::RunOutcome::Done
    );
    assert_eq!(
        t.browser.page(page).unwrap().stylesheet_usage().0,
        1,
        "test waited for response instead of media change"
    );
}
