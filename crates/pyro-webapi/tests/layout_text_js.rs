//! Layout-backed text and CSSOM behaviour exercised from JavaScript:
//! innerText from computed display, dataset names, document.images /
//! scripts, checkVisibility / scrollIntoView options, CSS.supports,
//! getComputedStyle pseudo-elements and CSS.registerProperty.

use std::cell::RefCell;
use std::rc::Rc;

use pyro_dom::parse::{parse_document, DomRef};
use pyro_dom::Dom;
use pyro_js::{ClassDef, Engine, JsClass, Realm, RealmOptions};
use pyro_webapi::*;

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
impl pyro_render::Fetcher for NoFetch {
    fn fetch(&self, url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(Err(format!("{url}: offline")));
    }
}

fn env(html: &str) -> Env {
    pyro_layout::install(std::sync::Arc::new(NoFetch));
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
fn transform_readback_separates_computed_and_resolved_values() {
    let env = env("<style>#target { width: 80px; transform: translateX(25%); zoom: 2 }</style><div id=target></div>");
    assert_eq!(eval(&env.main, r#"(() => {
        const target = document.querySelector('#target');
        return [getComputedStyle(target).transform, target.computedStyleMap().get('transform').toString()].join('|');
    })()"#), "matrix(1, 0, 0, 1, 20, 0)|translate(25%, 0px)");
}

#[test]
fn math_auto_preserves_dom_range_and_partial_node_text() {
    let env = env("<style>#outer { text-transform: math-auto }</style><div id=outer><span id=target>h</span></div>");
    let read = |source: &str, end: u32| eval(&env.main, &format!(r#"(() => {{
        const target = document.querySelector('#target');
        target.textContent = {source:?};
        const selection = getSelection();
        selection.setBaseAndExtent(target.firstChild, 0, target.firstChild, {end});
        return [target.innerText, selection.toString(), selection.getRangeAt(0).toString(), target.textContent].join('|');
    }})()"#));
    assert_eq!(read("h", 1), "ℎ|ℎ|h|h");
    assert_eq!(read("ab", 1), "ab|a|a|ab");
    assert_eq!(read("∇", 1), "𝛻|𝛻|∇|∇");
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

#[test]
fn css_supports_parses_with_stylo() {
    let e = env("<div></div>");
    let all = |cases: &str| eval(&e.main, &format!("[{cases}].map(c=>CSS.supports(...c)).join()"));
    assert_eq!(
        all("['color','red'],['display','block'],['margin','10px 20px'],['--my-color','blue'],['width','min(100px, 50%)'],\
             ['font','revert'],['COLOR','red'],['color','  red  '],['display','grid']"),
        "true,true,true,true,true,true,true,true,true"
    );
    assert_eq!(
        all("['not-a-real-property','red'],['color','rainbow'],['display','10px'],['  color  ','red'],\
             ['margin-top','!invalid'],['color','red !important'],['color','red; width: 1px'],['-moz-appearance','none'],\
             ['display','bogus'],['foo','bar']"),
        "false,false,false,false,false,false,false,false,false,false"
    );
    assert_eq!(
        all("['(color: red)'],['not (color: rainbow)'],['(color: red) and (display: block)'],\
             ['(color: red) or (not-a-real-property: red)'],['NOT (color: rainbow)'],['color: red'],\
             ['selector(a > b)'],['selector(:has(a))'],['(display: grid) and selector(div)']"),
        "true,true,true,true,true,true,true,true,true"
    );
    assert_eq!(
        all("['(color: rainbow)'],['(not-a-real-property: red)'],['not (display: grid)'],\
             ['(color: red) and (not-a-real-property: red)'],['not'],['(color: red) and'],['not-a-real-property: red'],\
             [';'],['selector(a >)'],['foo(bar)']"),
        "false,false,false,false,false,false,false,false,false,false"
    );
    assert_eq!(eval(&e.main, "try{CSS.supports();'no'}catch(err){err.constructor.name}"), "TypeError");
    // The namespace is made on first read and is then an ordinary data property.
    assert_eq!(
        eval(&e.main, "const d=Object.getOwnPropertyDescriptor(window,'CSS');[CSS===CSS,typeof d.value,d.enumerable,d.writable].join()"),
        "true,object,false,true"
    );
    e.page.dispose();
}

#[test]
fn get_computed_style_reads_pseudo_elements() {
    let e = env("<style>#t{width:100px;color:rgb(255,0,0)}#t::before{content:'icon';color:rgb(0,128,0);display:block}\
         #t::after{content:attr(data-x)}#n{display:none}#n::before{content:'Foo'}\
         #c::before{color:rgb(0,128,0)}li::marker{color:rgb(0,0,255)}input::placeholder{color:rgb(1,2,3)}</style>\
         <div id=t data-x=tail>x</div><div id=n></div><div id=c>c</div><p id=p>p</p><ul><li id=l>i</li></ul><input id=i placeholder=a>");
    let v = |js: &str| eval(&e.main, js);
    assert_eq!(
        v("[getComputedStyle(t,'::before').content,getComputedStyle(t,':before').color,getComputedStyle(t,'::before').display,\
           getComputedStyle(t,'::after').content,getComputedStyle(t,'::after').color].join('|')"),
        // Stylo substitutes attr() at computed-value time (Chrome serializes `attr(data-x)`).
        "\"icon\"|rgb(0, 128, 0)|block|\"tail\"|rgb(255, 0, 0)"
    );
    // No rule generates these: content none, other values inherited or initial.
    assert_eq!(
        v("[getComputedStyle(p,'::before').content,getComputedStyle(p,'::after').display,getComputedStyle(p,'::before').position].join('|')"),
        "none|inline|static"
    );
    assert_eq!(v("getComputedStyle(n,'::before').content"), "\"Foo\"");
    // A rule that generates no box (no `content`) still styles the pseudo-element.
    assert_eq!(
        v("[getComputedStyle(c,'::before').color,getComputedStyle(c,'::before').content,getComputedStyle(c,'::after').color].join('|')"),
        "rgb(0, 128, 0)|none|rgb(0, 0, 0)"
    );
    assert_eq!(v("getComputedStyle(l,'::marker').color"), "rgb(0, 0, 255)");
    assert_eq!(v("getComputedStyle(i,'::placeholder').color"), "rgb(1, 2, 3)");
    // Without a colon the argument is ignored; an unknown or malformed one is an empty style.
    assert_eq!(
        v("[getComputedStyle(t,'before').width,getComputedStyle(t,'nope').color].join('|')"),
        "100px|rgb(255, 0, 0)"
    );
    assert_eq!(
        v("['::nope',':nope','::before ','::before,::after','::\"before\"',':marker','::before0'].map(p=>{const s=getComputedStyle(t,p);return s.length+':'+s.color}).join()"),
        "0:,0:,0:,0:,0:,0:,0:"
    );
    assert_eq!(
        v("[getComputedStyle(t,':bef\\\\oRE').color,getComputedStyle(t,'::\\\\000041fter').content].join('|')"),
        "rgb(0, 128, 0)|\"tail\""
    );
    assert_eq!(v("getComputedStyle(t,'::before').length>0"), "true");
    e.page.dispose();
}

#[test]
fn css_register_property_validates_and_supplies_initial_values() {
    let e = env("<div id=t style='--set:4px'></div>");
    let v = |js: &str| eval(&e.main, js);
    let err = |def: &str| v(&format!("try{{CSS.registerProperty({def});'ok'}}catch(err){{err.name}}"));
    assert_eq!(err("{name:'--gap',syntax:'<length>',inherits:false,initialValue:'0px'}"), "ok");
    assert_eq!(err("{name:'--gap',syntax:'<length>',inherits:false,initialValue:'1px'}"), "InvalidModificationError");
    assert_eq!(err("{name:'--any',inherits:true}"), "ok");
    assert_eq!(err("{name:'nodash',inherits:false}"), "SyntaxError");
    assert_eq!(err("{name:'--bad-syntax',syntax:'<nope>',inherits:false,initialValue:'1'}"), "SyntaxError");
    assert_eq!(err("{name:'--no-initial',syntax:'<length>',inherits:false}"), "SyntaxError");
    assert_eq!(err("{name:'--wrong',syntax:'<length>',inherits:false,initialValue:'red'}"), "SyntaxError");
    assert_eq!(err("{name:'--relative',syntax:'<length>',inherits:false,initialValue:'1em'}"), "SyntaxError");
    assert_eq!(err("{name:'--keyword',syntax:'<length>',inherits:false,initialValue:'inherit'}"), "SyntaxError");
    assert_eq!(err("{syntax:'*',inherits:false}"), "TypeError");
    assert_eq!(err("{name:'--x'}"), "TypeError");
    assert_eq!(err("{name:'--set',syntax:'<length>',inherits:false,initialValue:'1px'}"), "ok");
    // An unset registered property reads its initial value; a set one its own.
    assert_eq!(
        v("const cs=getComputedStyle(t);[cs.getPropertyValue('--gap'),cs.getPropertyValue('--set'),cs.getPropertyValue('--any'),cs.getPropertyValue('--unregistered')].join('|')"),
        "0px|4px||"
    );
    e.page.dispose();
}
