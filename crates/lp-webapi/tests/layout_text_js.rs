//! Layout-backed text and CSSOM behaviour exercised from JavaScript:
//! innerText from computed display, dataset names, document.images /
//! scripts, checkVisibility / scrollIntoView options, CSS.supports,
//! getComputedStyle pseudo-elements and CSS.registerProperty.

use std::cell::RefCell;
use std::rc::Rc;

use lp_dom::parse::{parse_document, DomRef};
use lp_dom::Dom;
use lp_js::{ClassDef, Engine, JsClass, Realm, RealmOptions};
use lp_webapi::*;

struct TestWindowData {
    page: Rc<PageDom>,
    target: EventTargetData,
}

impl WindowHost for TestWindowData {
    fn page(&self) -> &Rc<PageDom> {
        &self.page
    }
    fn event_target(&self) -> &EventTargetData {
        &self.target
    }
}

struct TestWindow;
impl JsClass for TestWindow {
    const NAME: &'static str = "Window";
    type Data = TestWindowData;
    type Parent = EventTarget;
    fn define(def: &mut ClassDef<Self>) {
        def.upcast(|w: &TestWindowData| &w.target);
        define_members::<TestWindow>(def);
        def.getter("window", |s, _w| Ok(s.global().value()));
    }
}

struct Env {
    _engine: Engine,
    page: Rc<PageDom>,
    main: Realm,
}

struct NoFetch;
impl lp_render::Fetcher for NoFetch {
    fn fetch(&self, url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(Err(format!("{url}: offline")));
    }
}

fn env(html: &str) -> Env {
    lp_layout::install(std::sync::Arc::new(NoFetch));
    let engine = Engine::new().expect("engine");
    register(&engine);
    let window_class = engine.register::<TestWindow>();
    let dom: DomRef = Rc::new(RefCell::new(Dom::new()));
    let doc = parse_document(&dom, "http://localhost:5173/app", html);
    dom.borrow_mut().set_document_active(doc, true).unwrap();
    let page = PageDom::new(dom, doc);
    let mut vp = page.viewport();
    vp.width = 1440;
    vp.height = 900;
    page.set_viewport(vp);
    page.layout.borrow_mut().style_mut().discover_style_elements(&page.dom.borrow());
    let realm = engine
        .create_realm(RealmOptions { name: String::new(), global_class: Some(window_class), origin: None })
        .expect("realm");
    let key = page.window_target().unwrap_or_else(|| page.create_window_target(page.document()));
    realm
        .set_global_data::<TestWindow>(TestWindowData {
            page: page.clone(),
            target: EventTargetData::window(&page, key),
        })
        .unwrap();
    install(&realm.scope().unwrap(), page.clone()).unwrap();
    Env { _engine: engine, page, main: realm }
}

fn eval(realm: &Realm, src: &str) -> String {
    let s = realm.scope().unwrap();
    match s.eval(src, Some("test.js"), 1) {
        Ok(v) => v.to_string().unwrap(),
        Err(e) => format!("THROW {}", e.details(&s).text()),
    }
}

#[test]
fn inner_text_reads_computed_display() {
    let e = env("<div id=t></div>");
    let text = |html: &str| {
        eval(
            &e.main,
            &format!("(()=>{{const t=document.getElementById('t');t.innerHTML={html:?};return JSON.stringify(t.innerText)}})()"),
        )
    };
    for (html, expected) in [
        ("123<span style='display:block'>abc</span>def", r#""123\nabc\ndef""#),
        ("<div style='display:flex'><span>1</span><span>2</span></div>", r#""1\n2""#),
        ("<div style='display:grid'><span>1</span><span>2</span></div>", r#""1\n2""#),
        ("abc<span style='float:left'>123</span>def", r#""abc\n123\ndef""#),
        ("abc<span style='position:absolute'>123</span>def", r#""abc\n123\ndef""#),
        ("abc<span style='position:relative'>123</span>def", r#""abc123def""#),
        ("abc<div style='display:inline'>123</div>def", r#""abc123def""#),
        ("123<span style='display:contents'>abc</span>", r#""123abc""#),
        ("<span style='display:list-item'>a</span><span style='display:list-item'>b</span>", r#""a\nb""#),
        (
            "<div style='display:table'><span style='display:table-cell'>abc</span>\n<span style='display:table-cell'>def</span></div>",
            r#""abc\tdef""#,
        ),
        (
            "<div style='display:table'><span style='display:table-row'><span style='display:table-cell'>abc</span></span>\n<span style='display:table-row'><span style='display:table-cell'>def</span></span></div>",
            r#""abc\ndef""#,
        ),
        ("abc <img style='display:block'> def", r#""abc\ndef""#),
        ("123<p style='display:inline-block'>abc", r#""123\n\nabc""#),
        ("<template style='display:block'>abc</template>", r#""""#),
        ("<details><summary>abc</summary>123</details>", r#""abc""#),
        ("<details open><summary>abc</summary>123</details>", r#""abc\n123""#),
        ("<details><summary>s</summary><details open><summary>n</summary>x</details></details>", r#""s""#),
        ("a<option>123</option>bc", r#""a\n123\nbc""#),
        ("<select><option>abc</option><option>def</option></select>", r#""abc\ndef""#),
    ] {
        assert_eq!(text(html), expected, "{html}");
    }
    e.page.dispose();
}

#[test]
fn dataset_handles_non_ascii_names() {
    let e = env("<div id=t data-\u{e9}t\u{e9}=1></div>");
    assert_eq!(eval(&e.main, "const t=document.getElementById('t');t.dataset['\u{e9}t\u{e9}']"), "1");
    assert_eq!(eval(&e.main, "t.dataset['\u{e0}']='x';t.getAttribute('data-\u{e0}')"), "x");
    assert_eq!(eval(&e.main, "Object.keys(t.dataset).join()"), "\u{e9}t\u{e9},\u{e0}");
    assert_eq!(eval(&e.main, "delete t.dataset['\u{e0}'];t.hasAttribute('data-\u{e0}')"), "false");
    assert_eq!(eval(&e.main, "t.setAttribute('data-foo--bar','v');t.dataset['foo-Bar']"), "v");
    e.page.dispose();
}

#[test]
fn document_images_and_scripts_are_html_only() {
    let e = env("<img><svg><script></script><img/></svg><script></script>");
    assert_eq!(
        eval(&e.main, "document.body.append(document.createElementNS('x','img'));[document.images.length,document.scripts.length,document.getElementsByTagName('img').length].join()"),
        "2,1,3"
    );
    e.page.dispose();
}

#[test]
fn check_visibility_reads_its_options() {
    let e = env("<div id=v style='visibility:hidden'>a</div><div id=o style='opacity:0'>b</div>");
    assert_eq!(
        eval(
            &e.main,
            "[v.checkVisibility(),v.checkVisibility({checkVisibilityCSS:true}),v.checkVisibility({visibilityProperty:true}),\
             o.checkVisibility(),o.checkVisibility({checkOpacity:true}),o.checkVisibility({opacityProperty:true}),\
             o.checkVisibility(null)].join()"
        ),
        "true,false,false,true,false,false,true"
    );
    e.page.dispose();
}

#[test]
fn scroll_into_view_aligns_block_and_inline() {
    let e = env(
        "<body style='margin:0'><div style='height:3000px'></div>\
         <div id=t style='height:100px;width:100px;margin-left:3000px'>t</div><div style='height:3000px;width:8000px'></div></body>",
    );
    let top = "t.getBoundingClientRect().top+scrollY";
    let run = |arg: &str| eval(&e.main, &format!("scrollTo(0,0);t.scrollIntoView({arg});[scrollY,scrollX].join()"));
    // Element box at (3000, 3000), 100x100; viewport 1440x900.
    assert_eq!(eval(&e.main, &format!("{top}")), "3000");
    assert_eq!(run(""), run("{}"));
    assert_eq!(run("true").split(',').next(), Some("3000"));
    assert_eq!(run("false").split(',').next(), Some("2200"));
    assert_eq!(run("{block:'end'}").split(',').next(), Some("2200"));
    assert_eq!(run("{block:'center'}").split(',').next(), Some("2600"));
    assert_eq!(eval(&e.main, "scrollTo(0,2950);t.scrollIntoView({block:'nearest'});scrollY"), "2950");
    assert_eq!(eval(&e.main, "scrollTo(0,0);t.scrollIntoView({block:'nearest'});scrollY"), "2200");
    let x = |arg: &str| run(arg).split(',').nth(1).unwrap().to_owned();
    assert_eq!(x("{inline:'start'}"), "3000");
    assert_eq!(x("{inline:'center'}"), "2330");
    assert_eq!(x("{inline:'end'}"), "1660");
    assert_eq!(
        eval(&e.main, "try{t.scrollIntoView({block:'middle'});'no'}catch(err){err.constructor.name}"),
        "TypeError"
    );
    e.page.dispose();
}
