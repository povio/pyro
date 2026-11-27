//! Paint Timing (paint-timing/): `first-paint` and `first-contentful-paint` are decided at a
//! rendering update from what the document shows, with a presentation time later than the
//! paint time. Frames are asked for by the document itself: the pages here have no
//! `requestAnimationFrame` and no observer.
use pyro_browser::{BrowserThread, NavigateOpts, PageId, StaticLoader};
use std::rc::Rc;
use std::time::{Duration, Instant};

fn eval(t: &BrowserThread, page: PageId, source: &str) -> String {
    let s = t.browser.page(page).unwrap().main_frame.main_realm().unwrap().scope().unwrap();
    s.eval(source, None, 1).unwrap_or_else(|e| panic!("{}", e.details(&s).text())).to_string().unwrap()
}

fn settle(t: &mut BrowserThread) {
    assert_eq!(t.run_until_idle(Some(Instant::now() + Duration::from_secs(2))), pyro_runtime::RunOutcome::Idle);
}

fn load(html: &str) -> (BrowserThread, PageId) {
    let mut t = crate::browser_thread().unwrap();
    let loader = Rc::new(StaticLoader::default());
    loader.insert("http://site.test/page", "text/html", html, Duration::ZERO);
    t.browser.set_loader(Some(loader));
    let page = t.browser.create_page().unwrap();
    t.browser.navigate(page, NavigateOpts::address_bar("http://site.test/page")).unwrap();
    settle(&mut t);
    (t, page)
}

const NAMES: &str = "performance.getEntriesByType('paint').map(e => e.name).join()";

#[test]
fn text_paints_both_entries_with_a_presentation_after_the_paint() {
    let (t, page) = load("<body><p>hello</p></body>");
    assert_eq!(eval(&t, page, NAMES), "first-paint,first-contentful-paint");
    assert_eq!(
        eval(
            &t,
            page,
            "const [fp, fcp] = performance.getEntriesByType('paint'); \
             [fcp.presentationTime > fcp.paintTime, fcp.startTime === fcp.presentationTime, fp.startTime <= fcp.startTime, \
              fcp.toJSON().presentationTime === fcp.presentationTime].join()"
        ),
        "true,true,true,true"
    );
}

#[test]
fn an_empty_document_paints_nothing() {
    let (t, page) = load("<body></body>");
    assert_eq!(eval(&t, page, NAMES), "");
}

#[test]
fn whitespace_and_hidden_content_are_not_contentful() {
    let (t, page) = load(
        "<body><div>   </div><div style='opacity:0'>hidden</div><div style='visibility:hidden'>hidden</div>\
         <div style='display:none'>hidden</div><div style='position:relative;left:-5000px'>away</div></body>",
    );
    assert_eq!(eval(&t, page, NAMES), "");
}

#[test]
fn a_background_colour_is_a_first_paint_only() {
    let (t, page) = load("<body><div style='width:100px;height:100px;background:red'></div></body>");
    assert_eq!(eval(&t, page, NAMES), "first-paint");
    // A gradient is no content either.
    let (t, page) = load("<body><div style='width:100px;height:100px;background-image:linear-gradient(red,blue)'></div></body>");
    assert_eq!(eval(&t, page, NAMES), "first-paint");
}

#[test]
fn content_that_becomes_visible_reports_when_it_does() {
    let (mut t, page) = load("<body><div id=m style='opacity:0'>text</div></body>");
    assert_eq!(eval(&t, page, NAMES), "");
    eval(&t, page, "document.getElementById('m').style.opacity = '1'; ''");
    settle(&mut t);
    assert_eq!(eval(&t, page, NAMES), "first-paint,first-contentful-paint");
}
