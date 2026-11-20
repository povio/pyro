//! `load` and `error` of `<style>` elements (HTML "update a style block"),
//! as Chromium 147 fires them: in a later task, once the sheet applies and
//! the `@import`s it starts with are fetched. The expected orders are the
//! ones Chromium gave for the same markup.
use pyro_browser::{BrowserThread, InitScript, NavigateOpts, PageId, StaticLoader};
use pyro_dom::layout::{SubresourceBodies, SubresourceLoader};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// A layout fetcher with only the bodies of its document's network identity,
/// where the page's loader puts imported sheets (the binary's fetcher reads
/// them there before going to the network).
struct Provided(Option<Arc<SubresourceBodies>>);
impl pyro_render::Fetcher for Provided {
    fn fetch(&self, url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        let body = self.0.as_ref().and_then(|b| b.lock().get(&url).filter(|b| !b.is_empty()).map(|b| b.to_vec()));
        done(body.ok_or_else(|| format!("{url}: not provided")));
    }
}

fn eval(thread: &BrowserThread, page: PageId, source: &str) -> String {
    let realm = thread.browser.page(page).unwrap().main_frame.main_realm().unwrap();
    let scope = realm.scope().unwrap();
    scope.eval(source, None, 1).unwrap_or_else(|e| panic!("{}", e.details(&scope).text())).to_string().unwrap()
}
fn settle(thread: &mut BrowserThread) {
    assert_ne!(
        thread.run_until_idle(Some(Instant::now() + Duration::from_secs(3))),
        pyro_runtime::RunOutcome::TimedOut
    );
}
fn events(thread: &BrowserThread, page: PageId) -> String {
    eval(thread, page, "events.join('|')")
}
/// A page whose `events` records `<style>` loads and errors (by id), DOMContentLoaded and the window's load.
fn setup(html: &str) -> (BrowserThread, PageId, Rc<StaticLoader>) {
    pyro_layout::install_with(Rc::new(|loader: Option<&Arc<dyn SubresourceLoader>>| {
        Arc::new(Provided(loader.map(|l| l.bodies().clone()))) as Arc<dyn pyro_render::Fetcher>
    }));
    let mut thread = BrowserThread::new().unwrap();
    let page = thread.browser.create_page().unwrap();
    thread.browser.add_init_script(page, InitScript {
        id: "events".into(),
        world: None,
        source: "globalThis.events = []; for (const type of ['load', 'error']) document.addEventListener(type, e => { if (e.target.localName === 'style') events.push(e.target.id + ':' + type) }, true); document.addEventListener('DOMContentLoaded', () => events.push('dcl')); addEventListener('load', () => events.push('window:load'));".into(),
    });
    let loader = Rc::new(StaticLoader::default());
    loader.insert("http://site.test/", "text/html", html, Duration::ZERO);
    thread.browser.set_loader(Some(loader.clone()));
    (thread, page, loader)
}
fn load(html: &str, sheets: &[(&str, &str, u64)]) -> (BrowserThread, PageId, Rc<StaticLoader>) {
    let (mut thread, page, loader) = setup(html);
    for (path, css, delay) in sheets {
        loader.insert(&format!("http://site.test{path}"), "text/css", css, Duration::from_millis(*delay));
    }
    thread.browser.navigate(page, NavigateOpts::address_bar("http://site.test/")).unwrap();
    settle(&mut thread);
    (thread, page, loader)
}

#[test]
fn a_parsed_style_loads_in_a_later_task_before_dom_content_loaded() {
    let (thread, page, _) = load(
        "<style id=s onload=\"events.push('onload')\">html { display: none }</style><script>events.push('script')</script>",
        &[],
    );
    // Chromium: after-style-script, style-load, DCL, window-load.
    assert_eq!(events(&thread, page), "script|s:load|onload|dcl|window:load");
}

#[test]
fn media_and_disabled_do_not_matter_and_a_type_other_than_css_fires_nothing() {
    // An empty sheet, a `media` that does not match and a `disabled`
    // attribute (not one of <style>'s) load too; a `type` other than CSS
    // makes no sheet and fires nothing. Chromium: the same.
    let (thread, page, _) = load(
        "<style id=empty></style><style id=print media=print>p {}</style><style id=disabled disabled>p {}</style><style id=plain type=text/plain>p {}</style>",
        &[],
    );
    assert_eq!(events(&thread, page), "empty:load|print:load|disabled:load|dcl|window:load");
}

#[test]
fn a_style_waits_for_its_imports_and_blocks_the_parsers_scripts() {
    let (thread, page, loader) = load(
        "<style id=s>@import url(/a.css); p { color: red }</style><script>events.push('script:' + getComputedStyle(document.documentElement).display)</script>",
        &[("/a.css", "html { display: none }", 30)],
    );
    // Chromium: the script runs once the import applies, then the style loads.
    assert_eq!(events(&thread, page), "script:none|s:load|dcl|window:load");
    assert_eq!(loader.log.borrow().iter().filter(|u| u.ends_with("/a.css")).count(), 1);
}

#[test]
fn the_document_load_event_waits_for_imports_imported_sheets_included() {
    let (thread, page, loader) = load(
        "<style id=s>@import '/a.css';</style>",
        &[("/a.css", "@import url(b.css); p {}", 0), ("/b.css", "html { display: none }", 40)],
    );
    // Chromium: DCL, style-load, window-load, the nested import waited for.
    assert_eq!(events(&thread, page), "dcl|s:load|window:load");
    assert!(loader.log.borrow().iter().any(|u| u == "http://site.test/b.css"));
    assert_eq!(eval(&thread, page, "getComputedStyle(document.documentElement).display"), "none");
}

#[test]
fn a_failed_import_is_an_error_and_a_failed_import_of_an_imported_sheet_is_not() {
    let (thread, page, _) = load(
        "<style id=missing>@import '/missing.css';</style><style id=one>@import '/ok.css'; @import '/missing.css';</style><style id=nested>@import '/nest.css';</style>",
        &[("/ok.css", "p {}", 0), ("/nest.css", "@import '/missing.css';", 0)],
    );
    let all = events(&thread, page);
    assert!(all.ends_with("window:load"), "{all}");
    let mut styles: Vec<&str> = all.split('|').filter(|e| e.contains(':') && !e.starts_with("window")).collect();
    styles.sort();
    assert_eq!(styles, ["missing:error", "nested:load", "one:error"]);
}

#[test]
fn imports_that_do_not_count_are_not_fetched() {
    let (thread, page, loader) = load(
        "<style id=late>p { color: red } @import '/late.css';</style><style id=unsupported>@import '/unsupported.css' supports(nonsense: 1);</style>",
        &[],
    );
    assert_eq!(events(&thread, page), "late:load|unsupported:load|dcl|window:load");
    assert!(!loader.log.borrow().iter().any(|u| u.ends_with("late.css") || u.ends_with("unsupported.css")));
}

#[test]
fn script_inserted_styles_load_again_for_new_text_and_a_css_type_but_not_for_media() {
    let (mut thread, page, loader) = load("<p>x</p>", &[("/slow.css", "p {}", 60)]);
    eval(&thread, page, "events.length = 0; globalThis.s = document.createElement('style'); s.id = 'x'; s.textContent = 'p {}'; document.head.append(s); events.push('sync'); queueMicrotask(() => events.push('micro')); 1");
    settle(&mut thread);
    assert_eq!(events(&thread, page), "sync|micro|x:load");
    let step = |thread: &mut BrowserThread, script: &str| {
        eval(thread, page, &format!("events.length = 0; {script}; 1"));
        settle(thread);
        events(thread, page)
    };
    assert_eq!(step(&mut thread, "s.media = 'print'"), "");
    assert_eq!(step(&mut thread, "s.media = 'all'; s.textContent = 'a {}'"), "x:load");
    assert_eq!(step(&mut thread, "s.firstChild.data = 'b {}'"), "x:load");
    assert_eq!(step(&mut thread, "s.type = 'text/plain'; s.textContent = 'c {}'"), "");
    assert_eq!(step(&mut thread, "s.type = 'text/css'"), "x:load");
    // Removed before its task: it still fires (Chromium too).
    eval(&thread, page, "events.length = 0; globalThis.r = document.createElement('style'); r.onload = () => events.push('r:load:' + r.isConnected); document.head.append(r); 1");
    thread.browser.checkpoint();
    eval(&thread, page, "r.remove(); 1");
    settle(&mut thread);
    assert_eq!(events(&thread, page), "r:load:false");
    // Removed while its import loads: nothing fires, nothing waits.
    assert_eq!(
        step(&mut thread, "const i = document.createElement('style'); i.id = 'i'; i.textContent = '@import \"/slow.css\";'; document.head.append(i); setTimeout(() => i.remove(), 10)"),
        ""
    );
    assert!(loader.log.borrow().iter().any(|u| u.ends_with("/slow.css")));
    // Inserted with an import: it loads once the import arrives.
    assert_eq!(
        step(&mut thread, "const j = document.createElement('style'); j.id = 'j'; j.textContent = '@import \"/slow.css\";'; document.head.append(j); setTimeout(() => events.push('10ms'), 10)"),
        "10ms|j:load"
    );
}

#[test]
fn a_style_in_a_shadow_tree_or_a_frame_loads() {
    let (mut thread, page, _) = load("<div id=host></div><iframe srcdoc=\"<style onload='parent.events.push(&quot;frame:load&quot;)'>p {}</style>\"></iframe>", &[]);
    assert!(events(&thread, page).contains("frame:load"), "{}", events(&thread, page));
    // Not composed: the document does not see it.
    eval(&thread, page, "events.length = 0; const s = document.createElement('style'); s.onload = () => events.push('shadow:load'); s.textContent = 'p {}'; host.attachShadow({ mode: 'open' }).append(s); 1");
    settle(&mut thread);
    assert_eq!(events(&thread, page), "shadow:load");
}

/// `CSSImportRule.styleSheet` is the imported sheet once it loaded: its rules,
/// URL, owner rule and parent sheet; nested imports resolve against it
/// (css/cssom/cssimportrule.html, idlharness.html).
#[test]
fn an_import_rule_exposes_the_sheet_it_loaded() {
    let (thread, page, _loader) = load(
        "<style id=s>@import url(/a.css) screen;</style>",
        &[("/a.css", "@import 'b.css'; p { color: green }", 0), ("/b.css", "q {}", 0)],
    );
    assert_eq!(
        eval(
            &thread,
            page,
            "(() => { const rule = document.getElementById('s').sheet.cssRules[0], a = rule.styleSheet;
               const b = a.cssRules[0].styleSheet;
               return [a === rule.styleSheet, a.href, a.ownerRule === rule, a.parentStyleSheet === rule.parentStyleSheet,
                       a.ownerNode, a.media.mediaText, a.cssRules.length, a.cssRules[1].cssText, b && b.href,
                       b && b.cssRules[0].cssText].join('|'); })()"
        ),
        "true|http://site.test/a.css|true|true||screen|2|p { color: green; }|http://site.test/b.css|q { }"
    );
}

/// Serves through a static loader, answering `/a.css` as if redirected to `/dir/a.css` (the
/// response's URL).
struct Redirected(Rc<StaticLoader>);
impl pyro_browser::ResourceLoader for Redirected {
    fn fetch(&self, browser: &mut pyro_browser::Browser, request: pyro_browser::FetchRequest, done: pyro_browser::FetchDone) {
        let redirected = request.url.ends_with("/a.css");
        self.0.fetch(
            browser,
            request,
            Box::new(move |b, result| {
                done(b, result.map(|mut r| {
                    if redirected {
                        r.url = "http://site.test/dir/a.css".into();
                    }
                    r
                }))
            }),
        );
    }
}

/// CSSOM keeps an import's sheet under its request (URL and request URL modifiers): an
/// `@import` of the same URL whose `integrity()` fails has no sheet, though another import of
/// that URL loaded one (they shared one entry by URL). And an imported sheet that was redirected
/// resolves its own imports against its URL after redirects (security review cycle 1, MEDIUM 5).
#[test]
fn css_import_rules_keep_their_own_request_and_final_url() {
    let (mut thread, page, loader) = setup(
        "<style id=s>@import url(\"a.css\"); @import url(\"a.css\" integrity(\"sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=\"));</style>",
    );
    loader.insert("http://site.test/a.css", "text/css", "@import \"b.css\"; p { color: red }", Duration::ZERO);
    loader.insert("http://site.test/dir/b.css", "text/css", "p { margin: 0 }", Duration::ZERO);
    thread.browser.set_loader(Some(Rc::new(Redirected(loader.clone()))));
    thread.browser.navigate(page, NavigateOpts::address_bar("http://site.test/")).unwrap();
    settle(&mut thread);
    let rules = eval(
        &thread,
        page,
        "const r = document.getElementById('s').sheet.cssRules; \
         [r[0].styleSheet !== null, r[1].styleSheet === null, r[0].styleSheet && r[0].styleSheet.href].join(',')",
    );
    assert_eq!(rules, "true,true,http://site.test/dir/a.css");
    assert!(loader.log.borrow().iter().any(|u| u == "http://site.test/dir/b.css"), "{:?}", loader.log.borrow());
    assert!(!loader.log.borrow().iter().any(|u| u == "http://site.test/b.css"), "{:?}", loader.log.borrow());
}

/// CSSOM: a style sheet whose response is CORS-cross-origin is not origin-clean: reading its
/// `cssRules` or `rules` and calling `insertRule` or `deleteRule` throw a SecurityError. That
/// covers a `<link>`ed sheet from another origin fetched without CORS, an `@import`ed one, and a
/// redirect through another origin that ends at the document's own (a tainted response), and a
/// network error's sheet; a
/// same-origin sheet and a CORS one stay readable (security review cycle 2, HIGH 2).
#[test]
fn rules_of_a_sheet_that_is_not_origin_clean_throw() {
    struct Tainting(Rc<StaticLoader>);
    impl pyro_browser::ResourceLoader for Tainting {
        fn fetch(&self, browser: &mut pyro_browser::Browser, request: pyro_browser::FetchRequest, done: pyro_browser::FetchDone) {
            // `/tainted.css` came back through another origin (no CORS): opaque, at its own URL.
            let tainted = request.url.ends_with("/tainted.css");
            self.0.fetch(browser, request, Box::new(move |b, result| done(b, result.map(|mut r| {
                r.opaque |= tainted;
                r
            }))));
        }
    }
    let (mut thread, page, loader) = setup(
        "<link id=same rel=stylesheet href=same.css>\
         <link id=other rel=stylesheet href=http://other.test/a.css>\
         <link id=tainted rel=stylesheet href=tainted.css>\
         <link id=cors rel=stylesheet crossorigin href=http://other.test/cors.css>\
         <link id=failed rel=stylesheet href=missing.css>\
         <style id=s>@import url(\"http://other.test/i.css\");</style>",
    );
    for (url, css) in [
        ("http://site.test/same.css", "p { color: green }"),
        ("http://other.test/a.css", "p { color: red }"),
        ("http://site.test/tainted.css", "p { color: blue }"),
        ("http://other.test/cors.css", "p { margin: 0 }"),
        ("http://other.test/i.css", "p { padding: 0 }"),
    ] {
        loader.insert(url, "text/css", css, Duration::ZERO);
    }
    thread.browser.set_loader(Some(Rc::new(Tainting(loader.clone()))));
    thread.browser.navigate(page, NavigateOpts::address_bar("http://site.test/")).unwrap();
    settle(&mut thread);
    let probe = |id: &str| {
        eval(
            &thread,
            page,
            &format!(
                "(() => {{ const e = document.getElementById('{id}'); \
                 const sheet = e.localName === 'style' ? e.sheet.cssRules[0].styleSheet : e.sheet; \
                 if (!sheet) return 'no sheet'; \
                 return ['cssRules', 'rules', 'insertRule', 'deleteRule'].map(what => {{ try {{ \
                   if (what === 'insertRule') sheet.insertRule('q {{ }}', 0); \
                   else if (what === 'deleteRule') sheet.deleteRule(0); \
                   else sheet[what].length; \
                   return 'ok'; }} catch (x) {{ return x.name; }} }}).join(','); }})()"
            ),
        )
    };
    assert_eq!(probe("same"), "ok,ok,ok,ok");
    let thrown = "SecurityError,SecurityError,SecurityError,SecurityError";
    assert_eq!(probe("other"), thrown);
    assert_eq!(probe("tainted"), thrown);
    assert_eq!(probe("s"), thrown);
    // A network error is no CORS-same-origin response either (as Chrome: the sheet it leaves).
    assert_eq!(probe("failed"), thrown);
    // A CORS request: the static loader answers it as a CORS-same-origin response.
    assert_eq!(probe("cors"), "ok,ok,ok,ok");
}

/// Origin-cleanness is the sheet object's own response's, checked at every rule access: a
/// `CSSRuleList` kept from a clean `<link>` sheet (top level, or an `@media` rule's) sees none of
/// the rules of the cross-origin no-CORS sheet the link is repointed to, through `length`,
/// `item()` or an index; the link's `sheet` is a new object whose rules throw; an edit through
/// the old object changes nothing of the new. An `@import`ed sheet is never adoptable, a
/// readable parent's opaque import included (security review cycle 3, HIGH 2).
#[test]
fn a_repointed_link_gets_a_new_sheet_and_retained_rule_lists_see_none_of_it() {
    let (mut thread, page, loader) = setup(
        "<link id=l rel=stylesheet href=clean.css><style id=s>@import url(\"http://other.test/i.css\"); @import url(\"same.css\");</style>",
    );
    for (url, css) in [
        ("http://site.test/clean.css", "p { color: green } @media all { q { color: green } }"),
        ("http://other.test/secret.css", "p { color: red } @media all { q { --secret: 1 } } r { }"),
        ("http://other.test/i.css", "p { padding: 0 }"),
        ("http://site.test/same.css", "p { margin: 0 }"),
    ] {
        loader.insert(url, "text/css", css, Duration::ZERO);
    }
    thread.browser.navigate(page, NavigateOpts::address_bar("http://site.test/")).unwrap();
    settle(&mut thread);
    eval(
        &thread,
        page,
        "globalThis.old = document.getElementById('l').sheet; globalThis.kept = old.cssRules; \
         globalThis.inner = kept[1].cssRules; String(kept.length) + inner.length",
    );
    eval(
        &thread,
        page,
        "{ const link = document.getElementById('l'); globalThis.loaded = false; \
         link.addEventListener('load', () => loaded = true); link.addEventListener('error', () => loaded = true); \
         link.href = 'http://other.test/secret.css'; } ''",
    );
    for _ in 0..20 {
        settle(&mut thread);
        if eval(&thread, page, "String(loaded)") == "true" {
            break;
        }
    }
    assert_eq!(eval(&thread, page, "String(loaded)"), "true", "the repointed sheet loaded");
    let seen = eval(
        &thread,
        page,
        "globalThis.thrown = f => { try { f(); return 'ok'; } catch (e) { return e.name; } }; \
         globalThis.l = document.getElementById('l'); \
         [kept.length, kept.item(0), kept[0], inner.length, inner.item(0), l.sheet === old, \
          thrown(() => l.sheet.cssRules), thrown(() => l.sheet.cssRules.item(0)), \
          thrown(() => old.insertRule('s { --leak: 1 }', 0)), old.cssRules.length].join('|')",
    );
    assert_eq!(seen, "0|||0||false|SecurityError|SecurityError|ok|0");
    let adopted = eval(
        &thread,
        page,
        "globalThis.rules = document.getElementById('s').sheet.cssRules; \
         [thrown(() => rules[0].styleSheet.cssRules), thrown(() => { document.adoptedStyleSheets = [rules[0].styleSheet]; }), \
          thrown(() => { document.adoptedStyleSheets = [rules[1].styleSheet]; }), document.adoptedStyleSheets.length].join('|')",
    );
    assert_eq!(adopted, "SecurityError|NotAllowedError|NotAllowedError|0");
}
