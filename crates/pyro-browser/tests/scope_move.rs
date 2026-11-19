//! A `<style>` with `@scope` rules moved within the document: its prelude-less rules follow its new
//! parent (the implicit scope root, CSS Cascade 6 § 2.5), also when spelled in upper case.
use pyro_browser::{NavigateOpts, StaticLoader};
use std::rc::Rc;
use std::time::{Duration, Instant};

const PAGE: &str = r#"<!doctype html><body>
<div id=a><style id=s>@scope { .item { color: rgb(1, 0, 0) } }</style><p class=item id=pa>a</p></div>
<div id=b><p class=item id=pb>b</p></div>
<div id=c><style>@SCOPE { .item { color: rgb(2, 0, 0) } }</style><p class=item id=pc>c</p></div>
<p class=item id=out>o</p>
"#;

const STEPS: &str = r#"
const color = (id) => getComputedStyle(document.getElementById(id)).color;
const fails = [];
const expect = (label, id, want) => { const got = color(id); if (got !== want) fails.push(`${label}: ${id} ${got}`); };
expect('before', 'pa', 'rgb(1, 0, 0)');
expect('upper case', 'pc', 'rgb(2, 0, 0)');
expect('outside', 'out', 'rgb(0, 0, 0)');
document.getElementById('b').prepend(document.getElementById('s'));
expect('left behind', 'pa', 'rgb(0, 0, 0)');
expect('moved', 'pb', 'rgb(1, 0, 0)');
JSON.stringify(fails)
"#;

#[test]
fn moved_scope_style_follows_its_new_parent() {
    let mut thread = crate::browser_thread().unwrap();
    let page = thread.browser.create_page().unwrap();
    let loader = Rc::new(StaticLoader::default());
    loader.insert("http://site.test/", "text/html", PAGE, Duration::ZERO);
    thread.browser.set_loader(Some(loader));
    thread.browser.navigate(page, NavigateOpts::address_bar("http://site.test/")).unwrap();
    assert_ne!(thread.run_until_idle(Some(Instant::now() + Duration::from_secs(3))), pyro_runtime::RunOutcome::TimedOut);
    let realm = thread.browser.page(page).unwrap().main_frame.main_realm().unwrap();
    let scope = realm.scope().unwrap();
    let out = scope
        .eval(STEPS, None, 1)
        .unwrap_or_else(|e| panic!("{}", e.details(&scope).text()))
        .to_string()
        .unwrap();
    assert_eq!(out, "[]");
}
