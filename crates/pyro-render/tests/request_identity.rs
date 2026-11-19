//! Which request the renderer makes for a CSS image, and which response it uses: a mask or shape
//! image is fetched in CORS mode for the whole fetch, even from a same-origin URL (which may
//! redirect to another origin without CORS permission), and an image slot holds only the
//! response to its own request (URL, mode, integrity metadata), never one made otherwise.
use std::sync::{Arc, Mutex};

use pyro_render::*;

/// Serves `green.png` (20 by 20 opaque green). A CORS fetch fails, as one whose same-origin URL
/// redirected to another origin that grants no CORS permission does; a fetch with integrity
/// metadata fails (no digest matches). Records each request as (URL, how it was made).
#[derive(Default)]
struct Recorder(
    Mutex<Vec<(String, &'static str)>>,
    Mutex<Vec<(String, Option<String>)>>,
    Mutex<Vec<(String, Option<String>)>>,
);

fn green() -> Vec<u8> {
    encode_png(&RgbaImage { width: 20, height: 20, data: [0, 128, 0, 255].repeat(400) })
}

/// The fetcher the renderer holds: the shared record.
struct Shared(Arc<Recorder>);

impl Fetcher for Shared {
    fn fetch(&self, url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        self.0 .0.lock().unwrap().push((url.clone(), "no-cors"));
        let body = if url.ends_with("green.png") { Ok(green()) } else { Err("404".into()) };
        std::thread::spawn(move || done(body));
    }
    fn fetch_cors(&self, url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        self.0 .0.lock().unwrap().push((url, "cors"));
        std::thread::spawn(move || done(Err("CORS check failed after a redirect".into())));
    }
    fn fetch_with(&self, url: String, options: &FetchOptions, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        self.0 .1.lock().unwrap().push((url.clone(), options.client.clone()));
        self.0 .2.lock().unwrap().push((url.clone(), options.referrer.as_ref().map(|u| u.to_string())));
        let how = match (options.cors.is_some(), options.integrity.is_some()) {
            (_, true) => "integrity",
            (true, false) => "cors",
            (false, false) => "options",
        };
        self.0 .0.lock().unwrap().push((url.clone(), how));
        let body = if how == "options" && url.ends_with("green.png") { Ok(green()) } else { Err("failed".into()) };
        std::thread::spawn(move || done(body));
    }
}

fn render(body: &str) -> (RgbaImage, Vec<(String, &'static str)>) {
    let recorder = Arc::new(Recorder::default());
    let fetcher: Arc<dyn Fetcher> = Arc::new(Shared(recorder.clone()));
    let snapshot = Snapshot {
        html: format!("<!doctype html><body style='margin:0;background:#fff;width:200px;height:100px'>{body}"),
        base_url: "http://test/".into(),
        width: 200,
        height: 100,
        device_scale_factor: 1.0,
        ..Default::default()
    };
    let img = Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0;
    let log = recorder.0.lock().unwrap().clone();
    (img, log)
}

fn pixel(img: &RgbaImage, x: usize, y: usize) -> [u8; 3] {
    let i = (y * img.width as usize + x) * 4;
    [img.data[i], img.data[i + 1], img.data[i + 2]]
}

const WHITE: [u8; 3] = [255, 255, 255];
const GREEN: [u8; 3] = [0, 128, 0];

/// A same-origin `mask-image` is fetched in CORS mode (CSS Masking 1 § 6.2), and a response that
/// fails the check is no mask image: a transparent black layer that masks the box away. The
/// mode was dropped for a URL of the document's origin, so a redirect to another origin that
/// grants no CORS permission was used as the mask.
#[test]
fn a_same_origin_mask_image_is_fetched_in_cors_mode() {
    let (img, log) = render(
        "<div style='width:20px;height:20px;background:green;mask-image:url(green.png);mask-repeat:no-repeat'></div>",
    );
    assert!(log.iter().any(|(url, how)| url.ends_with("green.png") && *how == "cors"), "{log:?}");
    assert!(!log.iter().any(|(url, how)| url.ends_with("green.png") && *how == "no-cors"), "{log:?}");
    assert_eq!(pixel(&img, 10, 10), WHITE, "a mask that failed the CORS check masks the box away");
}

/// A same-origin `shape-outside` image is fetched in CORS mode too (CSS Shapes 1 § 4.1), never
/// served by a response fetched without CORS for an `<img>` of the same URL.
#[test]
fn a_same_origin_shape_image_is_fetched_in_cors_mode() {
    let (img, log) = render(
        "<img src=green.png style='display:block'>\
         <div style='float:left;width:20px;height:20px;shape-outside:url(green.png)'></div>",
    );
    assert!(log.iter().any(|(url, how)| url.ends_with("green.png") && *how == "cors"), "{log:?}");
    assert_eq!(pixel(&img, 10, 10), GREEN, "the <img> shows its own no-cors response");
}

/// An image with `integrity()` that no digest matches is not drawn, even where an element of the
/// same document draws the same URL fetched without integrity metadata: each slot holds the
/// response to its own request.
#[test]
fn an_image_that_fails_its_integrity_check_is_not_drawn() {
    let (img, log) = render(
        "<div style='width:20px;height:20px;background-image:url(green.png)'></div>\
         <div style='width:20px;height:20px;background-image:url(\"green.png\" integrity(\"sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=\"))'></div>",
    );
    assert!(log.iter().any(|(url, how)| url.ends_with("green.png") && *how == "integrity"), "{log:?}");
    assert_eq!(pixel(&img, 10, 10), GREEN, "the plain request's image");
    assert_eq!(pixel(&img, 10, 30), WHITE, "the image whose integrity check failed");
}

/// A persistent (mirrored) document: an element's image slot is kept across a style change only
/// for the same request. Adding an `integrity()` that no digest matches to the `url()` of an
/// image already loaded removes it: the slot was kept by its URL alone, with the old bytes,
/// which never passed the new check.
#[test]
fn a_changed_integrity_on_a_loaded_url_is_checked_again() {
    use pyro_dom::parse::{parse_document, DomRef};
    use pyro_dom::select::parse_selectors;
    use pyro_dom::snapshot::{RenderHtml, RenderNodeKind};
    use std::cell::RefCell;
    use std::rc::Rc;

    let dom: DomRef = Rc::new(RefCell::new(pyro_dom::Dom::new()));
    let doc = parse_document(
        &dom,
        "http://test/",
        "<!doctype html><body style='margin:0;background:#fff'>\
         <div id=d style='width:20px;height:20px;background-image:url(green.png)'></div>",
    );
    let opts = RenderHtml { style_text: &|_| None, stylesheet_href: &|_| None, mark: None, adopted: None };
    let tree = || {
        let t = dom.borrow().render_tree(doc, &opts).unwrap();
        Arc::new(Tree {
            nodes: t
                .nodes
                .into_iter()
                .map(|n| TreeNode {
                    key: n.key,
                    parent: n.parent,
                    kind: match n.kind {
                        RenderNodeKind::Element { name, attrs } => TreeKind::Element { name, attrs },
                        RenderNodeKind::Text(t) => TreeKind::Text(t),
                    },
                })
                .collect(),
        })
    };
    let snapshot = |tree: Arc<Tree>| Snapshot {
        base_url: "http://test/".into(),
        width: 100,
        height: 60,
        device_scale_factor: 1.0,
        resource_scope: "doc".into(),
        live: Some(Live { page: 1, tree, clip: None, interaction: None }),
        ..Default::default()
    };
    let recorder = Arc::new(Recorder::default());
    let fetcher: Arc<dyn Fetcher> = Arc::new(Shared(recorder.clone()));
    let mut renderer = Renderer::new();
    let first = renderer.render(&snapshot(tree()), Area::Viewport, Backend::Cpu, &fetcher).unwrap().0;
    assert_eq!(pixel(&first, 10, 10), GREEN, "the plain request's image");

    let d = dom.borrow().query_selector(doc, &parse_selectors("#d").unwrap()).unwrap().unwrap();
    dom.borrow_mut()
        .set_attribute(
            d,
            "style",
            "width:20px;height:20px;background-image:url(\"green.png\" integrity(\"sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=\"))",
        )
        .unwrap();
    let second = renderer.render(&snapshot(tree()), Area::Viewport, Backend::Cpu, &fetcher).unwrap().0;
    assert!(recorder.0.lock().unwrap().iter().any(|(_, how)| *how == "integrity"), "the new request is made");
    assert_eq!(pixel(&second, 10, 10), WHITE, "the old bytes never passed the new integrity check");
}

/// A frame element shows only the frame table entry its `data-pyro-frame` names, made for that
/// entry's client (a nested frame for its own entry's), so the browser sends its requests as that
/// frame's. Page attributes count for nothing: a `srcdoc` (any case) or a spoofed client next to
/// a genuine index, or on a frame naming no entry, loads no page content under any client (the
/// renderer read the client and the document from attributes; a page's namespaced `srcdoc`
/// written first won: security review cycle 3, HIGH 1).
#[test]
fn a_frame_shows_only_its_table_entry_for_its_client() {
    let mask = |path: &str| {
        format!("<div style='width:10px;height:10px;background:green;mask-image:url({path})'></div>")
    };
    let attr = |inner: &str| inner.replace('\'', "&#39;").replace('<', "&lt;").replace('>', "&gt;");
    let recorder = Arc::new(Recorder::default());
    let fetcher: Arc<dyn Fetcher> = Arc::new(Shared(recorder.clone()));
    let nested = FrameContent {
        html: mask("c/green.png"),
        url: "http://test/".into(),
        scroll: (0.0, 0.0),
        client: "NEST#L".into(),
        frames: Default::default(),
    };
    let first = FrameContent {
        html: format!("{}<iframe data-pyro-frame='2:0' srcdoc='{}'></iframe>", mask("a/green.png"), attr(&mask("evil/nested.png"))),
        url: "http://test/".into(),
        scroll: (0.0, 0.0),
        client: "F00D#L".into(),
        frames: Arc::new([("2".to_owned(), nested)].into_iter().collect()),
    };
    let snapshot = Snapshot {
        html: format!(
            "<!doctype html><body style='margin:0'>\
             <iframe data-pyro-frame='1:0' style='border:0;width:50px;height:20px'></iframe>\
             <iframe data-pyro-frame='9:0' data-pyro-frame-client='OTHER#L' srcdoc='{0}' SRCDOC='{0}' style='border:0;width:50px;height:20px'></iframe>\
             <iframe srcdoc='{1}' style='border:0;width:50px;height:20px'></iframe>",
            attr(&mask("evil/unknown.png")),
            attr(&mask("b/green.png")),
        ),
        frames: Arc::new([("1".to_owned(), first)].into_iter().collect()),
        base_url: "http://test/".into(),
        width: 200,
        height: 100,
        device_scale_factor: 1.0,
        ..Default::default()
    };
    Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap();
    let clients = recorder.1.lock().unwrap().clone();
    assert!(clients.iter().any(|(url, c)| url.ends_with("a/green.png") && c.as_deref() == Some("F00D#L")), "{clients:?}");
    assert!(clients.iter().any(|(url, c)| url.ends_with("c/green.png") && c.as_deref() == Some("NEST#L")), "{clients:?}");
    assert!(!clients.iter().any(|(url, _)| url.contains("evil")), "no page content in a table frame: {clients:?}");
    assert!(!clients.iter().any(|(_, c)| c.as_deref().is_some_and(|c| c.contains("OTHER"))), "no spoofed client: {clients:?}");
    // A frame of plain HTML (not from the snapshot) names no frame: never one of the table's.
    assert!(clients.iter().any(|(url, c)| url.ends_with("b/green.png") && c.as_deref() == Some("")), "{clients:?}");
}

/// A `url()`'s request has the style sheet it was written in as its referrer, whatever its mode
/// and modifiers, as Blink sends it (`CSSImageValue::PrepareFetch` sets the referrer string to the
/// sheet's URL unconditionally; WPT css-values `referrer-policy/unsafe-url/
/// url-image-referrer-policy-external-stylesheet` expects it for a no-cors image); an inline
/// sheet's leaves it to the document (the fetch's default). Plain `url()`s too.
#[test]
fn the_style_sheet_is_the_referrer_of_its_url_requests() {
    let recorder = Arc::new(Recorder::default());
    let fetcher: Arc<dyn Fetcher> = Arc::new(Shared(recorder.clone()));
    let mut sheets = std::collections::HashMap::new();
    sheets.insert(
        "http://sheets.test/dir/s.css".to_owned(),
        "#a { width: 10px; height: 10px; background-image: url(\"http://test/a.png\" referrer-policy(unsafe-url)) }\
         #b { width: 10px; height: 10px; background-image: url(\"http://test/b.png\" cross-origin(anonymous)) }\
         #c { width: 10px; height: 10px; background-image: url(\"http://test/c.png\") }"
            .to_owned(),
    );
    let snapshot = Snapshot {
        html: "<!doctype html><link rel=stylesheet href='http://sheets.test/dir/s.css'>\
               <style>#d { width: 10px; height: 10px; background-image: url(\"d.png\" referrer-policy(unsafe-url)) }</style>\
               <div id=a></div><div id=b></div><div id=c></div><div id=d></div>"
            .into(),
        base_url: "http://test/doc.html".into(),
        sheets,
        width: 100,
        height: 50,
        device_scale_factor: 1.0,
        ..Default::default()
    };
    Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap();
    let referrers = recorder.2.lock().unwrap().clone();
    let of = |name: &str| referrers.iter().find(|(url, _)| url.ends_with(name)).map(|(_, r)| r.clone());
    let sheet = Some(Some("http://sheets.test/dir/s.css".to_owned()));
    assert_eq!(of("a.png"), sheet, "no-cors with referrer-policy(): the sheet: {referrers:?}");
    assert_eq!(of("b.png"), sheet, "cors: the sheet: {referrers:?}");
    assert_eq!(of("c.png"), sheet, "no modifier: the sheet: {referrers:?}");
    assert_eq!(of("d.png"), Some(None), "inline style: the document: {referrers:?}");
}

/// Serves `a.css` as redirected to `http://elsewhere.test/dir/a.css`, whose body imports
/// `b.css`; records every URL asked for.
#[derive(Default)]
struct Redirecting(Mutex<Vec<String>>);
struct RedirectingFetcher(Arc<Redirecting>);
impl Fetcher for RedirectingFetcher {
    fn fetch(&self, url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        self.0 .0.lock().unwrap().push(url);
        done(Ok(Vec::new()));
    }
    fn fetch_final(
        &self,
        url: String,
        _options: &FetchOptions,
        done: Box<dyn FnOnce(Result<Fetched, String>) + Send>,
    ) {
        self.0 .0.lock().unwrap().push(url.clone());
        if url.ends_with("/a.css") {
            done(Ok(Fetched {
                body: b"@import \"b.css\";".to_vec(),
                url: "http://elsewhere.test/dir/a.css".into(),
                opaque: false,
            }));
        } else {
            done(Ok(Fetched { body: Vec::new(), url, opaque: false }));
        }
    }
}

/// An imported sheet that was redirected resolves its own `@import`s (and `url()`s) against its
/// URL after redirects, not the URL the `@import` named.
#[test]
fn a_redirected_import_resolves_against_its_final_url() {
    let log = Arc::new(Redirecting::default());
    let fetcher: Arc<dyn Fetcher> = Arc::new(RedirectingFetcher(log.clone()));
    let snapshot = Snapshot {
        html: "<!doctype html><style>@import \"a.css\";</style><p>x".into(),
        base_url: "http://test/".into(),
        width: 100,
        height: 50,
        device_scale_factor: 1.0,
        ..Default::default()
    };
    Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap();
    let asked = log.0.lock().unwrap().clone();
    assert!(asked.iter().any(|u| u == "http://elsewhere.test/dir/b.css"), "{asked:?}");
    assert!(!asked.iter().any(|u| u == "http://test/b.css"), "{asked:?}");
}
