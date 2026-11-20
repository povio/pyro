//! `<img>` loading through the browser: natural size from the file header,
//! load/error events, `complete`, window-load ordering and request reuse.
use pyro_browser::{BrowserThread, InitScript, NavigateOpts, PageId, StaticLoader};
use std::rc::Rc;
use std::time::{Duration, Instant};

fn eval(thread: &BrowserThread, page: PageId, source: &str) -> String {
    let realm = thread.browser.page(page).unwrap().main_frame.main_realm().unwrap();
    let scope = realm.scope().unwrap();
    scope.eval(source, None, 1).unwrap_or_else(|e| panic!("{}", e.details(&scope).text())).to_string().unwrap()
}
fn settle(thread: &mut BrowserThread) {
    assert_ne!(thread.run_until_idle(Some(Instant::now() + Duration::from_secs(3))), pyro_runtime::RunOutcome::TimedOut);
}
/// PNG signature + IHDR with the given size.
fn png(width: u32, height: u32) -> Vec<u8> {
    let mut b = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
    b.extend_from_slice(&width.to_be_bytes());
    b.extend_from_slice(&height.to_be_bytes());
    b.extend_from_slice(&[8, 6, 0, 0, 0]);
    b
}
fn setup(html: &str) -> (BrowserThread, PageId, Rc<StaticLoader>) {
    let mut thread = crate::browser_thread().unwrap();
    let page = thread.browser.create_page().unwrap();
    thread.browser.add_init_script(page, InitScript {
        id: "events".into(), world: None,
        source: "globalThis.events = []; for (const type of ['load', 'error']) document.addEventListener(type, e => { if (e.target.tagName === 'IMG') events.push(e.target.id + ':' + type + ':' + e.target.complete + ':' + e.target.naturalWidth + 'x' + e.target.naturalHeight) }, true); addEventListener('load', () => events.push('window:load'));".into(),
    });
    let loader = Rc::new(StaticLoader::default());
    loader.insert("http://site.test/", "text/html", html, Duration::ZERO);
    thread.browser.set_loader(Some(loader.clone()));
    (thread, page, loader)
}

#[test]
fn parsed_images_report_natural_size_and_delay_window_load() {
    let (mut thread, page, loader) = setup(
        "<img id=a src=/a.png><img id=b src=/missing.png><img id=c src=/page.html><img id=d src=/logo.svg><img id=e><img id=f src=''>",
    );
    loader.insert_bytes("http://site.test/a.png", "image/png", &png(3, 2), Duration::from_millis(30));
    loader.insert("http://site.test/page.html", "text/html", "<p>not an image</p>", Duration::ZERO);
    loader.insert(
        "http://site.test/logo.svg",
        "image/svg+xml",
        "<svg xmlns='http://www.w3.org/2000/svg' width='24' height='16'/>",
        Duration::ZERO,
    );
    thread.browser.navigate(page, NavigateOpts::address_bar("http://site.test/")).unwrap();
    settle(&mut thread);
    let events = eval(&thread, page, "events.join('|')");
    // Each image settles once with its final state; the slow PNG still precedes window load.
    for expected in ["a:load:true:3x2", "b:error:true:0x0", "c:error:true:0x0", "d:load:true:24x16", "f:error:true:0x0"]
    {
        assert!(events.contains(expected), "{expected} in {events}");
    }
    assert!(events.ends_with("window:load"), "{events}");
    assert_eq!(
        eval(
            &thread,
            page,
            "[document.getElementById('e').complete, document.getElementById('e').naturalWidth].join()"
        ),
        "true,0"
    );
    // One request per image source; no src and an empty src request nothing.
    assert_eq!(
        loader
            .log
            .borrow()
            .iter()
            .filter(|u| u.ends_with(".png") || u.ends_with(".svg") || u.ends_with("page.html"))
            .count(),
        4
    );
}

#[test]
fn svg_image_completion_preserves_available_html_image() {
    let (mut thread, page, loader) = setup("<img src='/asset.png'><svg id='svg'></svg>");
    loader.insert_bytes("http://site.test/asset.png", "image/png", &png(3, 2), Duration::ZERO);
    thread.browser.navigate(page, NavigateOpts::address_bar("http://site.test/")).unwrap();
    settle(&mut thread);
    eval(&thread, page, "const svgImage = document.createElementNS('http://www.w3.org/2000/svg', 'image'); svgImage.setAttribute('href','/asset.png'); document.querySelector('svg').appendChild(svgImage); 1");
    settle(&mut thread);
    assert_eq!(eval(&thread, page, "const copy = new Image(); copy.src='/asset.png'; [copy.complete, copy.naturalWidth].join()"), "true,3");
}

#[test]
fn available_image_checks_inherited_referrer_policy() {
    let (mut thread, page, loader) = setup("<!doctype html><img src='/asset.png'>");
    let url = "http://site.test/asset.png";
    loader.insert_bytes(url, "image/png", &png(3, 2), Duration::ZERO);
    thread.browser.navigate(page, NavigateOpts::address_bar("http://site.test/")).unwrap();
    settle(&mut thread);
    assert_eq!(loader.referer_of(url), Some(Some("http://site.test/".into())));
    eval(&thread, page, "const policy = document.createElement('meta'); policy.name='referrer'; policy.content='no-referrer'; document.head.append(policy); 1");
    settle(&mut thread);
    eval(&thread, page, "const next = new Image(); document.body.append(next); next.src='/asset.png'; 1");
    settle(&mut thread);
    assert_eq!(loader.log.borrow().iter().filter(|seen| *seen == url).count(), 2);
    assert_eq!(loader.referer_of(url), Some(None));
    assert_eq!(eval(&thread, page, "next.naturalWidth"), "3");
}

#[test]
fn src_changes_reload_once_and_report_pending_state() {
    let (mut thread, page, loader) = setup("<img id=a src=/a.png>");
    loader.insert_bytes("http://site.test/a.png", "image/png", &png(3, 2), Duration::ZERO);
    loader.insert_bytes("http://site.test/b.gif", "image/gif", b"GIF89a\x20\x03\x58\x02", Duration::from_millis(10));
    thread.browser.navigate(page, NavigateOpts::address_bar("http://site.test/")).unwrap();
    settle(&mut thread);
    // While the new source loads, complete is false and the old size is gone.
    assert_eq!(
        eval(&thread, page, "const a = document.getElementById('a'); a.src = '/b.gif'; a.src = '/b.gif'; [a.complete, a.naturalWidth].join()"),
        "false,3",
        "a new src is not complete at once; the old size stays until the new load"
    );
    settle(&mut thread);
    assert_eq!(
        eval(&thread, page, "events.filter(e => e.startsWith('a:')).join('|')"),
        "a:load:true:3x2|a:load:true:800x600"
    );
    // Setting the same source again does not request it again; as a relevant mutation it fires
    // `load` again, from the image already loaded (HTML's list of available images).
    eval(&thread, page, "document.getElementById('a').src = '/b.gif'; 1");
    settle(&mut thread);
    assert_eq!(loader.log.borrow().iter().filter(|u| u.ends_with("b.gif")).count(), 1);
    assert_eq!(eval(&thread, page, "events.filter(e => e.startsWith('a:')).length"), "3");
    // Removing src clears the image without an event.
    eval(&thread, page, "document.getElementById('a').removeAttribute('src'); 1");
    settle(&mut thread);
    assert_eq!(eval(&thread, page, "[document.getElementById('a').naturalWidth, events.length].join()"), "0,4");
}

#[test]
fn blob_and_inserted_images_load_after_window_load_without_reordering_it() {
    let (mut thread, page, _loader) = setup("<p>page</p>");
    thread.browser.navigate(page, NavigateOpts::address_bar("http://site.test/")).unwrap();
    settle(&mut thread);
    eval(
        &thread,
        page,
        "const bytes = new Uint8Array([0x47,0x49,0x46,0x38,0x39,0x61,0x05,0x00,0x07,0x00]); const img = document.createElement('img'); img.id = 'blob'; img.src = URL.createObjectURL(new Blob([bytes], {type: 'image/gif'})); document.body.append(img); 1",
    );
    // Incomplete as soon as src is set, before and after the load starts.
    assert_eq!(eval(&thread, page, "document.getElementById('blob').complete"), "false");
    thread.browser.checkpoint();
    assert_eq!(eval(&thread, page, "document.getElementById('blob').complete"), "false");
    settle(&mut thread);
    assert_eq!(eval(&thread, page, "events.join('|')"), "window:load|blob:load:true:5x7");
}

#[test]
fn images_that_are_not_connected_load_and_delay_window_load() {
    let (mut thread, page, loader) = setup(
        "<template id=t><img src=/inert.png></template><script>
           globalThis.detached = [];
           const img = new Image();
           img.onload = () => detached.push('new:' + img.complete + ':' + img.naturalWidth);
           img.src = '/slow.png';
           addEventListener('load', () => detached.push('window'));
           const built = document.createElement('img');
           built.onerror = () => detached.push('built:error');
           built.setAttribute('src', '/missing.png');
           const inert = document.implementation.createHTMLDocument().createElement('img');
           inert.src = '/inert-doc.png';
         </script>",
    );
    loader.insert_bytes("http://site.test/slow.png", "image/png", &png(4, 3), Duration::from_millis(30));
    thread.browser.navigate(page, NavigateOpts::address_bar("http://site.test/")).unwrap();
    settle(&mut thread);
    let log = eval(&thread, page, "detached.join('|')");
    assert!(log.ends_with("new:true:4|window"), "a detached image delays window load: {log}");
    assert!(log.contains("built:error"), "{log}");
    // Documents without a browsing context fetch nothing.
    assert!(!loader.log.borrow().iter().any(|u| u.contains("inert")), "{:?}", loader.log.borrow());
    // An image that is not referenced any more still fires its event.
    loader.insert_bytes("http://site.test/slow.png?2", "image/png", &png(1, 1), Duration::from_millis(20));
    eval(
        &thread,
        page,
        "detached.length = 0; (() => { const i = new Image(); i.onload = () => detached.push('gc'); i.src = '/slow.png?2'; })(); 1",
    );
    thread.browser.checkpoint();
    thread.browser.engine.collect_garbage_now();
    settle(&mut thread);
    assert_eq!(eval(&thread, page, "detached.join('|')"), "gc");
    // An image whose URL is in the list of available images is not requested again (HTML
    // "update the image data" step 7): complete at once, and `load` fires; nor is it when inserted.
    eval(
        &thread,
        page,
        "const again = new Image(); again.onload = () => detached.push('again'); again.src = '/slow.png'; \
         globalThis.again = again; globalThis.againComplete = again.complete + ':' + again.naturalWidth; 1",
    );
    settle(&mut thread);
    assert_eq!(eval(&thread, page, "againComplete"), "true:4");
    eval(&thread, page, "document.body.append(again); 1");
    settle(&mut thread);
    assert_eq!(loader.log.borrow().iter().filter(|u| u.ends_with("/slow.png")).count(), 1);
    assert!(eval(&thread, page, "detached.join('|')").ends_with("again"));
}

#[test]
fn decode_settles_with_the_current_request() {
    let (mut thread, page, loader) = setup("<p>decode</p>");
    loader.insert_bytes("http://site.test/a.png", "image/png", &png(2, 2), Duration::from_millis(10));
    loader.insert_bytes("http://site.test/b.png", "image/png", &png(3, 3), Duration::from_millis(10));
    loader.insert_bytes("http://site.test/corrupt.png", "image/png", b"\x89PNG-not-really", Duration::ZERO);
    thread.browser.navigate(page, NavigateOpts::address_bar("http://site.test/")).unwrap();
    settle(&mut thread);
    eval(
        &thread,
        page,
        "globalThis.out = {};
         globalThis.note = (name, p) => p.then(() => out[name] = 'ok', e => out[name] = e.name);
         const a = new Image(); a.src = '/a.png'; note('good', a.decode());
         note('nosrc', new Image().decode());
         note('missing', Object.assign(new Image(), {src: '/missing.png'}).decode());
         note('corrupt', Object.assign(new Image(), {src: '/corrupt.png'}).decode());
         const c = new Image(); c.src = '/a.png?changed'; note('changed', c.decode()); c.src = '/b.png'; note('after', c.decode());
         const inert = document.implementation.createHTMLDocument().createElement('img');
         inert.src = '/a.png'; note('inactive', inert.decode());
         globalThis.a = a; 1",
    );
    settle(&mut thread);
    assert_eq!(
        eval(
            &thread,
            page,
            "['good','nosrc','missing','corrupt','changed','after','inactive'].map(k => k + ':' + out[k]).join('|')"
        ),
        "good:ok|nosrc:EncodingError|missing:EncodingError|corrupt:EncodingError|changed:EncodingError|after:ok|inactive:EncodingError"
    );
    // A complete image decodes at once, also after setting the same src again.
    eval(&thread, page, "a.src = '/a.png'; note('again', a.decode()); 1");
    settle(&mut thread);
    assert_eq!(eval(&thread, page, "out.again"), "ok");
}

#[test]
fn base_href_resolves_image_urls() {
    let (mut thread, page, loader) = setup("");
    loader.insert(
        "http://site.test/deep/route/",
        "text/html",
        "<base href='/assets/'><img id=a src=a.png>",
        Duration::ZERO,
    );
    loader.insert_bytes("http://site.test/assets/a.png", "image/png", &png(5, 5), Duration::ZERO);
    thread.browser.navigate(page, NavigateOpts::address_bar("http://site.test/deep/route/")).unwrap();
    settle(&mut thread);
    assert_eq!(eval(&thread, page, "events.join('|')"), "a:load:true:5x5|window:load");
}

#[test]
fn decode_of_srcset_and_detached_picture_images() {
    let (mut thread, page, loader) = setup("<p>decode</p>");
    loader.insert_bytes("http://site.test/s.png", "image/png", &png(2, 2), Duration::from_millis(10));
    loader.insert_bytes("http://site.test/p.png", "image/png", &png(3, 3), Duration::from_millis(10));
    thread.browser.navigate(page, NavigateOpts::address_bar("http://site.test/")).unwrap();
    settle(&mut thread);
    eval(
        &thread,
        page,
        "globalThis.out = {};
         globalThis.note = (name, p) => p.then(() => out[name] = 'ok', e => out[name] = e.name);
         // srcset only (WPT image-decode.html): no src attribute.
         const s = new Image(); s.srcset = '/s.png 100w'; note('srcset', s.decode()); note('srcset2', s.decode());
         const m = new Image(); m.srcset = '/missing.png 100w'; note('srcsetMissing', m.decode());
         // A detached <picture> whose <source> changes (image-decode-picture.html).
         const picture = document.createElement('picture');
         const source = document.createElement('source');
         const img = document.createElement('img');
         picture.append(source, img);
         source.srcset = '/p.png';
         note('picture', img.decode());
         1",
    );
    settle(&mut thread);
    assert_eq!(
        eval(&thread, page, "['srcset','srcset2','srcsetMissing','picture'].map(k => k + ':' + out[k]).join('|')"),
        "srcset:ok|srcset2:ok|srcsetMissing:EncodingError|picture:ok"
    );
}

#[test]
fn images_removed_or_given_expandos_while_loading_still_fire_load() {
    let (mut thread, page, loader) = setup("<p>gc</p>");
    loader.insert_bytes("http://site.test/r.png", "image/png", &png(1, 1), Duration::from_millis(20));
    loader.insert_bytes("http://site.test/x.png", "image/png", &png(1, 1), Duration::from_millis(20));
    thread.browser.navigate(page, NavigateOpts::address_bar("http://site.test/")).unwrap();
    settle(&mut thread);
    eval(
        &thread,
        page,
        "globalThis.gc = [];
         (() => {
           const r = document.createElement('img'); r.onload = () => gc.push('removed');
           r.src = '/r.png'; document.body.append(r);
           const x = new Image(); x.onload = () => gc.push('expando'); x.src = '/x.png'; x.custom = 1;
         })(); 1",
    );
    thread.browser.checkpoint();
    eval(&thread, page, "document.querySelector('img[src=\"/r.png\"]').remove(); 1");
    thread.browser.checkpoint();
    thread.browser.engine.collect_garbage_now();
    settle(&mut thread);
    assert_eq!(eval(&thread, page, "gc.sort().join('|')"), "expando|removed");
}

/// A 1x1 opaque red PNG.
const RED_PNG: [u8; 70] = [
    137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0, 31, 21, 196,
    137, 0, 0, 0, 13, 73, 68, 65, 84, 120, 156, 99, 248, 207, 192, 240, 31, 0, 5, 0, 1, 255, 137, 153, 61, 29, 0, 0, 0,
    0, 73, 69, 78, 68, 174, 66, 96, 130,
];

#[test]
fn canvas_origin_clean_uses_the_documents_actual_origin() {
    let (mut thread, page, loader) = setup(
        "<canvas></canvas><img id=i src=/red.png>\
         <iframe sandbox=allow-scripts src=/child.html></iframe>\
         <iframe srcdoc='<canvas></canvas><img id=i src=/red.png>'></iframe>",
    );
    loader.insert_bytes("http://site.test/red.png", "image/png", &RED_PNG, Duration::ZERO);
    loader.insert(
        "http://site.test/child.html",
        "text/html",
        "<canvas></canvas><img id=i src=/red.png>",
        Duration::ZERO,
    );
    thread.browser.navigate(page, NavigateOpts::address_bar("http://site.test/")).unwrap();
    settle(&mut thread);
    let read = "(() => { const c = document.createElement('canvas'); c.width = c.height = 1; \
        const x = c.getContext('2d'); x.drawImage(document.getElementById('i'), 0, 0); \
        try { return String(x.getImageData(0, 0, 1, 1).data[0]) } catch (e) { return e.name } })()";
    let in_frame = |thread: &BrowserThread, url: &str| {
        let frame = thread.browser.frames(page).into_iter().find(|f| f.url == url).unwrap();
        let realm = thread.browser.frame(page, &frame.id).unwrap().main_realm().unwrap().clone();
        let scope = realm.scope().unwrap();
        scope.eval(read, None, 1).unwrap_or_else(|e| panic!("{}", e.details(&scope).text())).to_string().unwrap()
    };
    // Same-origin: readable.
    assert_eq!(eval(&thread, page, read), "255");
    // A sandboxed document's origin is opaque: the same-URL-origin image taints.
    assert_eq!(in_frame(&thread, "http://site.test/child.html"), "SecurityError");
    // about:srcdoc inherits the parent's origin: its same-origin image is clean.
    assert_eq!(in_frame(&thread, "about:srcdoc"), "255");
}

/// `<object data>` fetches its data: `load` for a fetched resource, `error` for a failed one, no
/// event without `data` or when the object is not being rendered (`display: none`, decided from
/// the last completed layout: here a layout read right after insertion), and a pending object
/// fetch delays the window's `load`.
#[test]
fn objects_fire_load_and_error_for_their_data() {
    let (mut thread, page, loader) = setup(
        "<script>globalThis.objects = []; for (const t of ['load', 'error']) document.addEventListener(t, e => { if (e.target.tagName === 'OBJECT') objects.push(e.target.id + ':' + t) }, true); addEventListener('load', () => objects.push('window:load'));</script>\
         <object id=a data=/a.svg></object><object id=b data=/missing.svg></object><object id=c></object>\
         <div style=display:none id=hide></div>\
         <script>const d = document.createElement('object'); d.id = 'd'; d.data = '/a.svg'; hide.append(d); document.body.offsetWidth;</script>",
    );
    loader.insert(
        "http://site.test/a.svg",
        "image/svg+xml",
        "<svg xmlns='http://www.w3.org/2000/svg' width='24' height='16'/>",
        Duration::from_millis(30),
    );
    thread.browser.navigate(page, NavigateOpts::address_bar("http://site.test/")).unwrap();
    settle(&mut thread);
    let events = eval(&thread, page, "objects.join('|')");
    assert!(events.contains("a:load") && events.contains("b:error"), "{events}");
    assert!(!events.contains("c:") && !events.contains("d:"), "{events}");
    assert!(events.ends_with("window:load"), "{events}");
}

/// A deferred module script runs as soon as its graph is ready, with no task in between: the
/// `load` of an `<object>` before it, whose resource is already there, has not fired yet when
/// the module adds its listener (WPT css/css-conditional/container-queries/crashtests/
/// chrome-bug-439886903-crash.html).
#[test]
fn a_deferred_module_runs_before_a_ready_object_loads() {
    let (mut thread, page, loader) = setup(
        "<object id=svg data=/a.svg></object>\
         <script type=module>\
           globalThis.order = ['module'];\
           await new Promise(r => svg.addEventListener('load', r, { once: true }));\
           order.push('object-load');\
         </script>",
    );
    loader.insert(
        "http://site.test/a.svg",
        "image/svg+xml",
        "<svg xmlns='http://www.w3.org/2000/svg' width='24' height='16'/>",
        Duration::ZERO,
    );
    thread.browser.navigate(page, NavigateOpts::address_bar("http://site.test/")).unwrap();
    settle(&mut thread);
    assert_eq!(eval(&thread, page, "order.join()"), "module,object-load");
}

/// `naturalWidth`/`naturalHeight` of SVG images follow CSS Images 3 (a missing dimension from the
/// ratio, else the default object size's), and `width`/`height` read the rendered size, or when
/// not rendered the attribute, else the natural size; a sourceless image reads 0.
#[test]
fn natural_and_rendered_sizes_of_images() {
    let (mut thread, page, loader) = setup(
        "<img id=a src=/none.svg style='display:none'><img id=b src=/ratio.svg style='display:none'><img id=c src=/w.svg>\
         <img id=d src=/a.png style='width:7px'>\
         <img id=e src=/a.png width=9 style='display:none'><img id=f src=/a.png style='display:none'>",
    );
    let svg = |attrs: &str| format!("<svg xmlns='http://www.w3.org/2000/svg' {attrs}/>");
    loader.insert("http://site.test/none.svg", "image/svg+xml", &svg(""), Duration::ZERO);
    loader.insert("http://site.test/ratio.svg", "image/svg+xml", &svg("viewBox='0 0 30 10'"), Duration::ZERO);
    loader.insert("http://site.test/w.svg", "image/svg+xml", &svg("width='60'"), Duration::ZERO);
    loader.insert_bytes("http://site.test/a.png", "image/png", &png(3, 2), Duration::ZERO);
    thread.browser.navigate(page, NavigateOpts::address_bar("http://site.test/")).unwrap();
    settle(&mut thread);
    assert_eq!(
        eval(
            &thread,
            page,
            "[...document.images].map(i => [i.id, i.naturalWidth, i.naturalHeight, i.width, i.height].join(' ')).join('|')"
        ),
        "a 300 150 300 150|b 300 100 300 100|c 60 150 60 150|d 3 2 7 4|e 3 2 9 2|f 3 2 3 2"
    );
    assert_eq!(eval(&thread, page, "const f = document.getElementById('f'); f.removeAttribute('src'); [f.width, f.height].join()"), "0,0");
}

/// HTML relevant mutations: setting `src` (even to its value) or changing the CORS settings state
/// restarts the request and fires `load` again; a `sizes` change without `w` descriptors or a
/// `crossorigin` value of the same state does not.
#[test]
fn relevant_mutations_restart_the_request() {
    let (mut thread, page, loader) = setup("<img id=a src=/a.png>");
    loader.insert_bytes("http://site.test/a.png", "image/png", &png(3, 2), Duration::ZERO);
    thread.browser.navigate(page, NavigateOpts::address_bar("http://site.test/")).unwrap();
    settle(&mut thread);
    let step = |thread: &mut BrowserThread, js: &str| {
        eval(thread, page, &format!("events.length = 0; {{ const a = document.getElementById('a'); {js} }}"));
        settle(thread);
        eval(thread, page, "events.join('|')")
    };
    assert_eq!(step(&mut thread, "a.src = a.getAttribute('src');"), "a:load:true:3x2");
    assert_eq!(step(&mut thread, "a.crossOrigin = 'anonymous';"), "a:load:true:3x2");
    assert_eq!(step(&mut thread, "a.crossOrigin = 'foo';"), "");
    assert_eq!(step(&mut thread, "a.sizes = '10px';"), "");
}

/// `sizes="auto"` of a lazy image is its laid-out width; a lazy image that is not rendered
/// selects (its `auto` entry skipped) without fetching until it is rendered, or until it stops
/// being lazy (then the source selected while deferred is fetched). "Rendered" and the width come
/// from the last completed layout, never running one per image: here a layout read right after
/// the insertion.
#[test]
fn sizes_auto_and_deferred_lazy_images() {
    let (mut thread, page, loader) = setup(
        "<body><script>document.body.innerHTML = \"<img id=a loading=lazy sizes=auto style='width:10px' srcset='/s.png 50w, /l.png 51w'>\
         <img id=b loading=lazy sizes='auto, 500px' style='display:none;width:10px' srcset='/s.png?b 50w, /l.png?b 500w'>\"; document.body.offsetWidth;</script>",
    );
    for url in ["s.png", "l.png", "s.png?b", "l.png?b"] {
        loader.insert_bytes(&format!("http://site.test/{url}"), "image/png", &png(3, 2), Duration::ZERO);
    }
    thread.browser.navigate(page, NavigateOpts::address_bar("http://site.test/")).unwrap();
    settle(&mut thread);
    let fetched = |loader: &StaticLoader, needle: &str| loader.log.borrow().iter().any(|u| u.ends_with(needle));
    assert_eq!(eval(&thread, page, "document.getElementById('a').currentSrc"), "http://site.test/s.png");
    assert!(!fetched(&loader, "?b"), "a display:none lazy image is not fetched");
    eval(&thread, page, "document.getElementById('b').loading = 'eager'; 1");
    settle(&mut thread);
    assert!(fetched(&loader, "l.png?b"), "eager fetches the source selected while deferred (auto skipped: 500px)");
}

/// Serves through a static loader, marking the response of URLs with `detour` in them opaque (as
/// pyro-net reports a no-cors redirect chain that went to another origin and came back).
struct Opaquing(Rc<StaticLoader>);
impl pyro_browser::ResourceLoader for Opaquing {
    fn fetch(&self, browser: &mut pyro_browser::Browser, request: pyro_browser::FetchRequest, done: pyro_browser::FetchDone) {
        let detour = request.url.contains("detour");
        self.0.fetch(
            browser,
            request,
            Box::new(move |b, result| {
                done(b, result.map(|mut r| {
                    r.opaque |= detour;
                    r
                }))
            }),
        );
    }
}

/// An image whose response is opaque taints a canvas it is drawn on, though its URL is of the
/// document's origin: the response's tainting decides origin-cleanness, not the final URL
/// (security review cycle 1, HIGH 3). Its entry in the list of available images is not clean
/// either.
#[test]
fn an_opaque_image_at_the_documents_origin_taints_a_canvas() {
    let (mut thread, page, loader) = setup(
        "<canvas id=c width=4 height=4></canvas><script>\
           globalThis.read = [];\
           function draw(src) { const i = new Image(); i.onload = () => {\
             const x = document.getElementById('c').getContext('2d'); x.drawImage(i, 0, 0);\
             try { const d = x.getImageData(0, 0, 1, 1).data; read.push(src + ':clean:' + d[3]); } catch (e) { read.push(src + ':' + e.name); }\
           }; i.src = src; }\
           draw('/plain.png'); draw('/detour.png');\
         </script>",
    );
    // A whole 2x2 opaque red PNG (`png()` writes only a header, which no canvas decodes).
    const RED: &[u8] = &[
        137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 2, 0, 0, 0, 2, 8, 6, 0, 0, 0, 114, 182, 13,
        36, 0, 0, 0, 17, 73, 68, 65, 84, 120, 156, 99, 248, 207, 192, 240, 31, 132, 25, 96, 12, 0, 71, 202, 7, 249, 103,
        89, 110, 183, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
    ];
    loader.insert_bytes("http://site.test/plain.png", "image/png", RED, Duration::ZERO);
    loader.insert_bytes("http://site.test/detour.png", "image/png", RED, Duration::ZERO);
    thread.browser.set_loader(Some(Rc::new(Opaquing(loader.clone()))));
    thread.browser.navigate(page, NavigateOpts::address_bar("http://site.test/")).unwrap();
    settle(&mut thread);
    let read = eval(&thread, page, "read.sort().join('|')");
    assert!(read.starts_with("/detour.png:SecurityError|"), "{read}");
    assert!(read.contains("/plain.png:clean:255") || read.contains("/plain.png:SecurityError"), "{read}");
}

/// A loaded SVG `<image>` draws its raster to a canvas (the list of available images has it;
/// it drew nothing), clean when same-origin, tainting the canvas when its response is opaque
/// (security review cycle 1, finding 7).
#[test]
fn an_svg_image_draws_to_a_canvas() {
    let (mut thread, page, loader) = setup(
        "<canvas id=c width=4 height=4></canvas>\
         <svg><image id=p href=/plain.png width=2 height=2 /><image id=d href=/detour.png width=2 height=2 /></svg><script>\
           globalThis.read = [];\
           addEventListener('load', () => { for (const id of ['p', 'd']) {\
             const x = document.getElementById('c').getContext('2d'); x.clearRect(0, 0, 4, 4);\
             x.drawImage(document.getElementById(id), 0, 0);\
             try { read.push(id + ':' + x.getImageData(0, 0, 1, 1).data[3]); } catch (e) { read.push(id + ':' + e.name); }\
           } });\
         </script>",
    );
    const RED: &[u8] = &[
        137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 2, 0, 0, 0, 2, 8, 6, 0, 0, 0, 114, 182, 13,
        36, 0, 0, 0, 17, 73, 68, 65, 84, 120, 156, 99, 248, 207, 192, 240, 31, 132, 25, 96, 12, 0, 71, 202, 7, 249, 103,
        89, 110, 183, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
    ];
    loader.insert_bytes("http://site.test/plain.png", "image/png", RED, Duration::ZERO);
    loader.insert_bytes("http://site.test/detour.png", "image/png", RED, Duration::ZERO);
    thread.browser.set_loader(Some(Rc::new(Opaquing(loader.clone()))));
    thread.browser.navigate(page, NavigateOpts::address_bar("http://site.test/")).unwrap();
    settle(&mut thread);
    assert_eq!(eval(&thread, page, "read.join('|')"), "p:255|d:SecurityError");
}

#[test]
fn completed_image_body_is_ready_for_layout() {
    let (mut thread, page, loader) = setup("<img src='/detour.png'>");
    let url = "http://site.test/detour.png";
    let image = png(3, 2);
    loader.insert_bytes(url, "image/png", &image, Duration::ZERO);
    thread.browser.set_loader(Some(Rc::new(Opaquing(loader.clone()))));
    thread.browser.navigate(page, NavigateOpts::address_bar("http://site.test/")).unwrap();
    settle(&mut thread);
    assert_eq!(eval(&thread, page, "document.images[0].naturalWidth"), "3");
    let client = pyro_browser::document_client(&thread.browser.page(page).unwrap().main_frame);
    let bodies = thread.browser.layout_bodies(page).unwrap();
    let key = bodies.effective_key(pyro_dom::layout::LayoutBodyKey::new(&client, url, &Default::default(), pyro_dom::layout::SubresourceKind::Image));
    assert!(!bodies.known(&key));
    let bytes = thread.browser.page(page).unwrap().main_frame.page_dom.as_ref().unwrap().available_image(url, 0).unwrap().bytes.unwrap().0;
    for count in [1, 2] {
        let bytes = bytes.clone();
        thread.browser.fetch_subresource(page, url.into(), pyro_browser::ResourceKind::Image, Box::new(move |_, result| {
            let image = result.unwrap();
            assert_eq!(std::sync::Arc::ptr_eq(&bytes, &image.body), count == 1);
            assert!(image.opaque);
        }));
        settle(&mut thread);
        assert_eq!(loader.log.borrow().iter().filter(|entry| *entry == url).count(), count);
    }
}

#[test]
fn obsolete_image_slot_is_removed_even_when_replacement_fails_or_is_too_large() {
    let (mut thread, page, loader) = setup("<img id=a src=/small.png>");
    let small = "http://site.test/small.png";
    let mut large = png(3, 2);
    loader.insert_bytes(small, "image/png", &large, Duration::ZERO);
    large.resize(16 * 1024 * 1024 + 1, 0);
    loader.insert_bytes("http://site.test/large.png", "image/png", &large, Duration::ZERO);
    thread.browser.navigate(page, NavigateOpts::address_bar("http://site.test/")).unwrap();
    settle(&mut thread);
    let client = pyro_browser::document_client(&thread.browser.page(page).unwrap().main_frame);
    let bodies = thread.browser.layout_bodies(page).unwrap();
    let key = bodies.effective_key(pyro_dom::layout::LayoutBodyKey::new(&client, small, &Default::default(), pyro_dom::layout::SubresourceKind::Image));
    for replacement in ["missing.png", "large.png"] {
        assert!(bodies.start(&key));
        bodies.finish(key.clone(), pyro_dom::layout::LayoutBody::local(small, png(3, 2)));
        eval(&thread, page, &format!("document.images[0].src='/{replacement}'"));
        settle(&mut thread);
        assert!(!bodies.known(&key), "{replacement}");
        eval(&thread, page, "document.images[0].src='/small.png'");
        settle(&mut thread);
    }
}

#[test]
fn image_handoff_budget_and_detachment_bound_allocations() {
    let (mut thread, page, loader) = setup("<img src='/one'><img src='/two'><img src='/three'>");
    let mut large = png(3, 2);
    large.resize(6 * 1024 * 1024, 0);
    for name in ["one", "two", "three"] {
        loader.insert_bytes(&format!("http://site.test/{name}"), "image/png", &large, Duration::ZERO);
    }
    thread.browser.navigate(page, NavigateOpts::address_bar("http://site.test/")).unwrap();
    settle(&mut thread);
    eval(&thread, page, "globalThis.detached = document.images[2]; detached.remove()");
    settle(&mut thread);
    for (name, expected) in [("one", 2), ("two", 1), ("three", 2)] {
        let url = format!("http://site.test/{name}");
        thread.browser.fetch_subresource(page, url.clone(), pyro_browser::ResourceKind::Image, Box::new(|_, response| {
            assert_eq!(response.unwrap().body.len(), 6 * 1024 * 1024);
        }));
        settle(&mut thread);
        assert_eq!(loader.log.borrow().iter().filter(|seen| **seen == url).count(), expected, "{name}");
    }
}

/// What the page fetched itself and hands its layout (an `<object>`'s image) is stored for the
/// requesting document alone, with its URL after redirects, tainting and status written together:
/// no other document's key finds it; a CORS SVG `<image>`'s response and a failed one are never
/// stored; a navigation leaves nothing of the old document. `<object>` bytes went to a bare-URL
/// slot every document of the identity read, with tainting and final URL kept apart (security
/// review cycles 2-3, layout body cache).
#[test]
fn the_page_hands_its_layout_bodies_for_its_document_alone() {
    let (mut thread, page, loader) = setup(
        "<object id=o data=/detour.png></object><object id=missing data=/missing.png></object>\
         <svg><image id=s crossorigin=anonymous href=/cors.png width=1 height=1></image></svg>",
    );
    loader.insert_bytes("http://site.test/detour.png", "image/png", &png(3, 2), Duration::ZERO);
    loader.insert_bytes("http://site.test/cors.png", "image/png", &png(1, 1), Duration::ZERO);
    loader.insert("http://site.test/next", "text/html", "<p>next</p>", Duration::ZERO);
    thread.browser.set_loader(Some(Rc::new(Opaquing(loader.clone()))));
    thread.browser.navigate(page, NavigateOpts::address_bar("http://site.test/")).unwrap();
    settle(&mut thread);
    let client = pyro_browser::document_client(&thread.browser.page(page).unwrap().main_frame);
    let bodies = thread.browser.layout_bodies(page).unwrap();
    let key = |document: &str, url: &str| bodies.effective_key(pyro_dom::layout::LayoutBodyKey::new(document, url, &Default::default(), pyro_dom::layout::SubresourceKind::Image));
    let object = bodies.get(&key(&client, "http://site.test/detour.png")).expect("the object's body, for its document");
    assert_eq!((object.final_url.as_str(), object.opaque, object.status), ("http://site.test/detour.png", true, 200));
    assert!(bodies.get(&key("OTHER#doc", "http://site.test/detour.png")).is_none());
    // The key is the request's as sent, destination included: not the same URL used as a sheet.
    let as_sheet = pyro_dom::layout::LayoutBodyKey::new(
        &client,
        "http://site.test/detour.png",
        &Default::default(),
        pyro_dom::layout::SubresourceKind::Style,
    );
    assert!(bodies.get(&as_sheet).is_none(), "cycle 4, 3b");
    assert!(bodies.get(&key(&client, "http://site.test/missing.png")).is_none(), "a failed response is not stored");
    assert!(bodies.get(&key(&client, "http://site.test/cors.png")).is_none(), "a CORS response is not a no-cors body");
    thread.browser.navigate(page, NavigateOpts::address_bar("http://site.test/next")).unwrap();
    settle(&mut thread);
    assert!(bodies.get(&key(&client, "http://site.test/detour.png")).is_none(), "gone with its document");
}

/// Changing only `crossorigin` on an SVG `<image>` is a new request (its mode, its tainting): it
/// is fetched again in CORS mode and drawn from that response, not kept with the old image and
/// its taint (a URL-only check returned early; the attribute was no relevant mutation of an SVG
/// `<image>`: security review cycles 2-3, 2d).
#[test]
fn an_svg_images_crossorigin_change_is_a_new_request() {
    let (mut thread, page, loader) = setup(
        "<canvas id=c width=4 height=4></canvas><svg><image id=d href=/detour.png width=2 height=2 /></svg><script>\
           globalThis.draw = () => { const c = document.createElement('canvas'); c.width = c.height = 4; const x = c.getContext('2d');\
             x.drawImage(document.getElementById('d'), 0, 0);\
             try { return String(x.getImageData(0, 0, 1, 1).data[3]); } catch (e) { return e.name; } };\
         </script>",
    );
    const RED: &[u8] = &[
        137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 2, 0, 0, 0, 2, 8, 6, 0, 0, 0, 114, 182, 13,
        36, 0, 0, 0, 17, 73, 68, 65, 84, 120, 156, 99, 248, 207, 192, 240, 31, 132, 25, 96, 12, 0, 71, 202, 7, 249, 103,
        89, 110, 183, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
    ];
    loader.insert_bytes("http://site.test/detour.png", "image/png", RED, Duration::ZERO);
    thread.browser.set_loader(Some(Rc::new(Opaquing(loader.clone()))));
    thread.browser.navigate(page, NavigateOpts::address_bar("http://site.test/")).unwrap();
    settle(&mut thread);
    assert_eq!(eval(&thread, page, "draw()"), "SecurityError", "the no-cors detour taints");
    eval(&thread, page, "document.getElementById('d').setAttribute('crossorigin', 'anonymous'); 1");
    settle(&mut thread);
    let fetches = loader.log.borrow().iter().filter(|u| u.ends_with("/detour.png")).count();
    assert_eq!(fetches, 2, "fetched again for the new mode");
    assert_eq!(eval(&thread, page, "draw()"), "255", "drawn from the CORS response");
}


/// A frame that goes leaves nothing in the layout bodies: what its document's page handed its
/// layout is evicted when the iframe is removed (only navigation forgot before: security review
/// cycle 4, 3d).
#[test]
fn a_removed_frames_bodies_are_evicted() {
    let (mut thread, page, loader) = setup("<iframe id=f src=/child></iframe>");
    loader.insert("http://site.test/child", "text/html", "<object data=/c.png></object>", Duration::ZERO);
    loader.insert_bytes("http://site.test/c.png", "image/png", &png(2, 2), Duration::ZERO);
    thread.browser.navigate(page, NavigateOpts::address_bar("http://site.test/")).unwrap();
    settle(&mut thread);
    let frames = thread.browser.frames(page);
    let child = frames.iter().find(|f| f.parent.is_some()).expect("the child frame");
    let client = format!("{}#{}", child.id.0, child.loader_id.0);
    let bodies = thread.browser.layout_bodies(page).unwrap();
    let key = bodies.effective_key(pyro_dom::layout::LayoutBodyKey::new(&client, "http://site.test/c.png", &Default::default(), pyro_dom::layout::SubresourceKind::Image));
    assert!(bodies.get(&key).is_some(), "the child's object body, for the child");
    eval(&thread, page, "document.getElementById('f').remove(); 1");
    settle(&mut thread);
    assert!(bodies.get(&key).is_none(), "gone with its frame");
}
