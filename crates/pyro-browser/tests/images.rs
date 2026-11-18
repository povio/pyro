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
    // Setting the same source again does not request it again.
    eval(&thread, page, "document.getElementById('a').src = '/b.gif'; 1");
    settle(&mut thread);
    assert_eq!(loader.log.borrow().iter().filter(|u| u.ends_with("b.gif")).count(), 1);
    // Removing src clears the image without an event.
    eval(&thread, page, "document.getElementById('a').removeAttribute('src'); 1");
    settle(&mut thread);
    assert_eq!(eval(&thread, page, "[document.getElementById('a').naturalWidth, events.length].join()"), "0,3");
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
    // Inserting a loaded image does not request it again.
    eval(&thread, page, "const again = new Image(); again.src = '/slow.png'; globalThis.again = again; 1");
    settle(&mut thread);
    eval(&thread, page, "document.body.append(again); 1");
    settle(&mut thread);
    assert_eq!(loader.log.borrow().iter().filter(|u| u.ends_with("/slow.png")).count(), 2);
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
/// event without `data` or when the object is not being rendered (`display: none`), and a
/// pending object fetch delays the window's `load`.
#[test]
fn objects_fire_load_and_error_for_their_data() {
    let (mut thread, page, loader) = setup(
        "<script>globalThis.objects = []; for (const t of ['load', 'error']) document.addEventListener(t, e => { if (e.target.tagName === 'OBJECT') objects.push(e.target.id + ':' + t) }, true); addEventListener('load', () => objects.push('window:load'));</script>\
         <object id=a data=/a.svg></object><object id=b data=/missing.svg></object><object id=c></object>\
         <div style=display:none><object id=d data=/a.svg></object></div>",
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
