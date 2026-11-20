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
        version: 7,
        sheets: Default::default(),
    };
    let first = FrameContent {
        html: format!(
            "{}<iframe data-pyro-frame='N2:2:0000000000000007' srcdoc='{}'></iframe><iframe data-pyro-frame='WRONG:2:0000000000000007'></iframe>",
            mask("a/green.png"),
            attr(&mask("evil/nested.png"))
        ),
        url: "http://test/".into(),
        scroll: (0.0, 0.0),
        client: "F00D#L".into(),
        frames: Arc::new(FrameTableData { nonce: "N2".into(), entries: [("2".to_owned(), nested)].into_iter().collect() }),
        version: 1,
        sheets: Default::default(),
    };
    let snapshot = Snapshot {
        html: format!(
            "<!doctype html><body style='margin:0'>\
             <iframe data-pyro-frame='N1:1:0000000000000001' style='border:0;width:50px;height:20px'></iframe>\
             <iframe data-pyro-frame='N1:9:0000000000000001' data-pyro-frame-client='OTHER#L' srcdoc='{0}' SRCDOC='{0}' style='border:0;width:50px;height:20px'></iframe>\
             <iframe data-pyro-frame='N1:1:0000000000000002' srcdoc='{0}' style='border:0;width:50px;height:20px'></iframe>\
             <iframe data-pyro-frame='1:0000000000000001' srcdoc='{0}' style='border:0;width:50px;height:20px'></iframe>\
             <iframe srcdoc='{1}' style='border:0;width:50px;height:20px'></iframe>",
            attr(&mask("evil/unknown.png")),
            attr(&mask("b/green.png")),
        ),
        frames: Arc::new(FrameTableData { nonce: "N1".into(), entries: [("1".to_owned(), first)].into_iter().collect() }),
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
    // An unmarked frame of a snapshot document loads nothing at all (cycle 5, 1).
    assert!(!clients.iter().any(|(url, _)| url.ends_with("b/green.png")), "{clients:?}");
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
        "S".to_owned(),
        "#a { width: 10px; height: 10px; background-image: url(\"http://test/a.png\" referrer-policy(unsafe-url)) }\
         #b { width: 10px; height: 10px; background-image: url(\"http://test/b.png\" cross-origin(anonymous)) }\
         #c { width: 10px; height: 10px; background-image: url(\"http://test/c.png\") }"
            .to_owned(),
    );
    let snapshot = Snapshot {
        html: "<!doctype html><link rel=stylesheet href='http://sheets.test/dir/s.css' data-pyro-sheet=S>\
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

/// An external sheet at the document's own URL is an external sheet: its `url()`s have it as
/// their referrer. Inline CSS is told by the sheet's identity, never by comparing its URL with
/// the document's (which made this sheet's requests the document's: security review cycle 3, B2).
#[test]
fn an_external_sheet_at_the_documents_url_is_still_the_referrer() {
    let recorder = Arc::new(Recorder::default());
    let fetcher: Arc<dyn Fetcher> = Arc::new(Shared(recorder.clone()));
    let mut sheets = std::collections::HashMap::new();
    sheets.insert(
        "D".to_owned(),
        "#e { width: 10px; height: 10px; background-image: url(\"e.png\") }".to_owned(),
    );
    let snapshot = Snapshot {
        html: "<!doctype html><link rel=stylesheet href='http://test/doc.html' data-pyro-sheet=D>\
               <style>#d { width: 10px; height: 10px; background-image: url(\"d.png\" referrer-policy(unsafe-url)) }</style>\
               <div id=e></div><div id=d></div><div id=s style='width:10px;height:10px;background-image:url(s.png)'></div>"
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
    assert_eq!(of("e.png"), Some(Some("http://test/doc.html".to_owned())), "the external sheet: {referrers:?}");
    assert_eq!(of("d.png"), Some(None), "inline style: the document: {referrers:?}");
    assert_eq!(of("s.png"), None, "a style attribute's plain url() is a default request: {referrers:?}");
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
                referrer_policy: None,
            }));
        } else {
            done(Ok(Fetched { body: Vec::new(), url, opaque: false, referrer_policy: None }));
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

/// A frame document's sheets are its own: two frames whose sheets share a URL but say different
/// things each paint with their own (they were merged into one map by URL, first one winning);
/// a client the fetcher does not admit (no live document of the snapshot) gets nothing, not even
/// a sheet the snapshot carries (security review cycle 4, 3a).
#[test]
fn each_frame_is_served_its_own_sheets_and_only_admitted_clients() {
    struct Admitting(Arc<Recorder>, &'static str);
    impl Fetcher for Admitting {
        fn admits(&self, client: Option<&str>) -> bool {
            client != Some(self.1)
        }
        fn fetch(&self, url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
            Shared(self.0.clone()).fetch(url, done)
        }
    }
    let frame = |client: &str, color: &str| {
        let mut sheets = std::collections::BTreeMap::new();
        sheets.insert("F".to_owned(), format!("body {{ margin: 0; background: {color} }}"));
        FrameContent {
            html: "<!doctype html><link rel=stylesheet href='http://test/s.css' data-pyro-sheet=F><body>".into(),
            url: "http://test/".into(),
            scroll: (0.0, 0.0),
            client: client.into(),
            frames: Default::default(),
            version: 1,
            sheets,
        }
    };
    let snapshot = Snapshot {
        html: "<!doctype html><body style='margin:0;background:#fff'>\
               <iframe data-pyro-frame='N:1:0000000000000001' style='border:0;width:40px;height:20px;display:block'></iframe>\
               <iframe data-pyro-frame='N:2:0000000000000001' style='border:0;width:40px;height:20px;display:block'></iframe>\
               <iframe data-pyro-frame='N:3:0000000000000001' style='border:0;width:40px;height:20px;display:block'></iframe>"
            .into(),
        frames: Arc::new(FrameTableData {
            nonce: "N".into(),
            entries: [
                ("1".to_owned(), frame("A#L", "rgb(0, 128, 0)")),
                ("2".to_owned(), frame("B#L", "rgb(0, 0, 255)")),
                ("3".to_owned(), frame("GONE#L", "rgb(255, 0, 0)")),
            ]
            .into_iter()
            .collect(),
        }),
        base_url: "http://test/".into(),
        width: 40,
        height: 60,
        device_scale_factor: 1.0,
        ..Default::default()
    };
    let fetcher: Arc<dyn Fetcher> = Arc::new(Admitting(Arc::new(Recorder::default()), "GONE#L"));
    let image = Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0;
    assert_eq!(pixel(&image, 10, 10), [0, 128, 0], "frame A's own sheet");
    assert_eq!(pixel(&image, 10, 30), [0, 0, 255], "frame B's own sheet, at the same URL");
    assert_eq!(pixel(&image, 10, 50), [255, 255, 255], "a client not admitted gets nothing");
}

/// A `url()`'s request is sent under the policy of the sheet object it was written in: a linked
/// sheet's from its link (the snapshot's engine attribute), an imported sheet's from its own
/// response; never by its URL, so the same URL as another document's sheet with another policy
/// changes nothing (it was selected by URL, `no-referrer` where two responses disagreed:
/// security review cycle 4, S1).
#[test]
fn a_urls_policy_is_its_sheet_objects() {
    #[derive(Default)]
    struct Policies(Mutex<Vec<(String, Option<String>)>>);
    struct Serving(Arc<Policies>);
    impl Fetcher for Serving {
        fn fetch(&self, url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
            self.0 .0.lock().unwrap().push((url, None));
            done(Err("404".into()));
        }
        fn fetch_final(&self, url: String, options: &FetchOptions, done: Box<dyn FnOnce(Result<Fetched, String>) + Send>) {
            self.0 .0.lock().unwrap().push((url.clone(), options.referrer_policy.clone()));
            if url.ends_with("/imp.css") {
                done(Ok(Fetched {
                    body: b"#i { width: 10px; height: 10px; background-image: url(http://test/i.png) }".to_vec(),
                    url,
                    opaque: false,
                    referrer_policy: Some("no-referrer".into()),
                }));
            } else {
                done(Err("404".into()));
            }
        }
    }
    let mut sheets = std::collections::HashMap::new();
    sheets.insert(
        "P".to_owned(),
        "@import url(http://test/imp.css); #a { width: 10px; height: 10px; background-image: url(http://test/a.png) }".to_owned(),
    );
    let mut frame_sheets = std::collections::BTreeMap::new();
    frame_sheets.insert(
        "Q".to_owned(),
        "#b { width: 10px; height: 10px; background-image: url(http://test/b.png) }".to_owned(),
    );
    let frame = FrameContent {
        html: "<!doctype html><link rel=stylesheet href='http://test/s.css' data-pyro-sheet=Q><div id=b></div>".into(),
        url: "http://test/".into(),
        scroll: (0.0, 0.0),
        client: "F#L".into(),
        frames: Default::default(),
        version: 1,
        sheets: frame_sheets,
    };
    let snapshot = Snapshot {
        html: "<!doctype html><link rel=stylesheet href='http://test/s.css' data-pyro-sheet=P data-pyro-sheet-referrer-policy='origin' data-pyro-n=N>\
               <div id=a></div><div id=i></div>\
               <iframe data-pyro-frame='N:1:0000000000000001' style='border:0;width:20px;height:20px'></iframe>"
            .into(),
        sheets,
        frames: Arc::new(FrameTableData { nonce: "N".into(), entries: [("1".to_owned(), frame)].into_iter().collect() }),
        base_url: "http://test/doc.html".into(),
        width: 100,
        height: 50,
        device_scale_factor: 1.0,
        ..Default::default()
    };
    let policies = Arc::new(Policies::default());
    let fetcher: Arc<dyn Fetcher> = Arc::new(Serving(policies.clone()));
    Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap();
    let seen = policies.0.lock().unwrap().clone();
    let of = |name: &str| seen.iter().find(|(url, _)| url.ends_with(name)).map(|(_, p)| p.clone());
    assert_eq!(of("/a.png"), Some(Some("origin".to_owned())), "the link's sheet's: {seen:?}");
    assert_eq!(of("/imp.css"), Some(Some("origin".to_owned())), "its import, sent by it: {seen:?}");
    assert_eq!(of("/i.png"), Some(Some("no-referrer".to_owned())), "the imported sheet's own: {seen:?}");
    assert_eq!(of("/b.png"), Some(None), "a frame's sheet at the same URL has its own (none): {seen:?}");
}

/// Two documents at one URL with the same inline sheet share its parsed form (one per thread);
/// the second document's `url()`s are still its own CSS's (the document as referrer): the shared
/// sheet's URL data, the first document's object, is adopted as the second's own. It counted as
/// an external sheet, so its requests named the document explicitly and were another request than
/// layout asked for (shape-outside images raced `load`: cycle 4 reliability).
#[test]
fn a_shared_inline_sheet_is_each_documents_own_css() {
    let recorder = Arc::new(Recorder::default());
    let fetcher: Arc<dyn Fetcher> = Arc::new(Shared(recorder.clone()));
    let snapshot = Snapshot {
        html: "<!doctype html><style>#d { width: 10px; height: 10px; background-image: url(\"shared.png\" referrer-policy(unsafe-url)) }</style><div id=d></div>"
            .into(),
        base_url: "http://test/shared-doc.html".into(),
        width: 40,
        height: 20,
        device_scale_factor: 1.0,
        ..Default::default()
    };
    Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap();
    Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap();
    let referrers: Vec<Option<String>> =
        recorder.2.lock().unwrap().iter().filter(|(url, _)| url.ends_with("shared.png")).map(|(_, r)| r.clone()).collect();
    assert_eq!(referrers, [None, None], "both documents' own CSS");
}

/// In a render snapshot's document (it has a frame table), a frame element without a valid marker
/// loads nothing, whatever attributes it has: a `src` (a page's namespaced `src` that markup made
/// plain), a `srcdoc`, an `<object data>` document; the renderer fetched an unmarked iframe's
/// `src` itself (security review cycle 5, 1).
#[test]
fn an_unmarked_frame_of_a_snapshot_loads_nothing() {
    let recorder = Arc::new(Recorder::default());
    let fetcher: Arc<dyn Fetcher> = Arc::new(Shared(recorder.clone()));
    let snapshot = Snapshot {
        html: "<!doctype html><body>\
               <iframe src='http://evil.test/page.html'></iframe>\
               <iframe srcdoc=\"<div style='width:9px;height:9px;background-image:url(http://evil.test/s.png)'></div>\"></iframe>\
               <object type='text/html' data='http://evil.test/doc.html' width=20 height=20></object>"
            .into(),
        frames: Arc::new(FrameTableData { nonce: "N".into(), entries: Default::default() }),
        base_url: "http://test/".into(),
        width: 100,
        height: 50,
        device_scale_factor: 1.0,
        ..Default::default()
    };
    Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap();
    let asked: Vec<String> = recorder.0.lock().unwrap().iter().map(|(url, _)| url.clone()).collect();
    assert!(!asked.iter().any(|u| u.contains("page.html") || u.contains("s.png")), "{asked:?}");
}

/// The renderer serves a linked sheet by its link (the snapshot's sheet key), never by URL: two
/// links whose responses ended at one URL each apply their own text; a request that is no such
/// style request at that URL (an image) never gets the sheet's bytes (security review cycle 5, 2).
#[test]
fn sheets_are_served_by_their_link_never_by_url() {
    let recorder = Arc::new(Recorder::default());
    let fetcher: Arc<dyn Fetcher> = Arc::new(Shared(recorder.clone()));
    let mut sheets = std::collections::HashMap::new();
    sheets.insert("A".to_owned(), "#a { width: 10px; height: 10px; background: rgb(0, 128, 0) }".to_owned());
    sheets.insert("B".to_owned(), "#b { width: 10px; height: 10px; background: rgb(0, 0, 255) }".to_owned());
    let snapshot = Snapshot {
        html: "<!doctype html><body style='margin:0;background:#fff'>\
               <link rel=stylesheet href='http://test/s.css' data-pyro-sheet=A>\
               <link rel=stylesheet href='http://test/s.css' data-pyro-sheet=B>\
               <div id=a></div><div id=b></div><img src='http://test/s.css' width=1 height=1>"
            .into(),
        sheets,
        base_url: "http://test/".into(),
        width: 40,
        height: 40,
        device_scale_factor: 1.0,
        ..Default::default()
    };
    let image = Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0;
    assert_eq!(pixel(&image, 5, 5), [0, 128, 0], "the first link's own text");
    assert_eq!(pixel(&image, 5, 15), [0, 0, 255], "the second link's own text, at the same URL");
    let asked: Vec<String> = recorder.0.lock().unwrap().iter().map(|(u, _)| u.clone()).collect();
    assert!(asked.iter().any(|u| u == "http://test/s.css"), "the image went to the network: {asked:?}");
}

/// A linked sheet the renderer fetched itself (no snapshot text for it) and that was redirected
/// resolves its URLs against its URL after redirects, not the URL the link asked for (security
/// review cycle 5, 5).
#[test]
fn a_redirected_linked_sheet_resolves_against_its_final_url_on_the_network() {
    #[derive(Default)]
    struct Seen(Mutex<Vec<String>>);
    struct Redirect(Arc<Seen>);
    impl Fetcher for Redirect {
        fn fetch(&self, url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
            self.0 .0.lock().unwrap().push(url);
            done(Err("404".into()));
        }
        fn fetch_final(&self, url: String, _options: &FetchOptions, done: Box<dyn FnOnce(Result<Fetched, String>) + Send>) {
            self.0 .0.lock().unwrap().push(url.clone());
            if url.ends_with("/old/a.css") {
                done(Ok(Fetched {
                    body: b"#x { width: 10px; height: 10px; background-image: url(img.png) }".to_vec(),
                    url: "http://test/new/a.css".into(),
                    opaque: false,
                    referrer_policy: None,
                }));
            } else {
                done(Err("404".into()));
            }
        }
    }
    let seen = Arc::new(Seen::default());
    let fetcher: Arc<dyn Fetcher> = Arc::new(Redirect(seen.clone()));
    let snapshot = Snapshot {
        html: "<!doctype html><link rel=stylesheet href='http://test/old/a.css'><div id=x></div>".into(),
        base_url: "http://test/".into(),
        width: 40,
        height: 40,
        device_scale_factor: 1.0,
        ..Default::default()
    };
    Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap();
    let asked = seen.0.lock().unwrap().clone();
    assert!(asked.iter().any(|u| u == "http://test/new/img.png"), "{asked:?}");
    assert!(!asked.iter().any(|u| u == "http://test/old/img.png"), "{asked:?}");
}

/// A `<pre>` the snapshot wrote for a `<plaintext>` or `<xmp>` (pyro-dom `TAG_ATTRIBUTE`) is that
/// element in the render document (its own selectors match it), and the elements after it are
/// elements: a later image still loads (security review cycle 5, 7).
#[test]
fn a_plaintext_written_as_pre_is_itself_and_ends_where_the_dom_does() {
    #[derive(Default)]
    struct Seen(Mutex<Vec<String>>);
    struct Record(Arc<Seen>);
    impl Fetcher for Record {
        fn fetch(&self, url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
            self.0 .0.lock().unwrap().push(url);
            done(Err("404".into()));
        }
    }
    let seen = Arc::new(Seen::default());
    let fetcher: Arc<dyn Fetcher> = Arc::new(Record(seen.clone()));
    let snapshot = Snapshot {
        html: "<!doctype html><style>plaintext + xmp + div { width: 10px; height: 10px; background-image: url(http://test/sib.png) }\
               pre ~ div { background-image: url(http://test/pre.png) !important }</style>\
               <pre data-pyro-tag=\"plaintext\">\na&lt;b</pre><pre data-pyro-tag=\"xmp\">\nx</pre><div></div>"
            .into(),
        base_url: "http://test/".into(),
        width: 40,
        height: 40,
        device_scale_factor: 1.0,
        ..Default::default()
    };
    Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap();
    let asked = seen.0.lock().unwrap().clone();
    assert!(asked.iter().any(|u| u == "http://test/sib.png"), "{asked:?}");
    assert!(!asked.iter().any(|u| u == "http://test/pre.png"), "{asked:?}");
}

/// In a render snapshot's document an engine attribute counts only on an element carrying the
/// snapshot's nonce (`data-pyro-n`): without it, a `<pre>`'s `data-pyro-tag` names nothing and a
/// link's `data-pyro-sheet` serves no snapshot sheet, so markup a reparse produced from page text
/// cannot act as the engine's (security review cycle 6, 1).
#[test]
fn engine_attributes_without_the_snapshots_nonce_are_inert() {
    #[derive(Default)]
    struct Seen(Mutex<Vec<String>>);
    struct Record(Arc<Seen>);
    impl Fetcher for Record {
        fn fetch(&self, url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
            self.0 .0.lock().unwrap().push(url);
            done(Err("404".into()));
        }
    }
    let render = |marker: &str| {
        let seen = Arc::new(Seen::default());
        let fetcher: Arc<dyn Fetcher> = Arc::new(Record(seen.clone()));
        let snapshot = Snapshot {
            html: format!(
                "<!doctype html><style>div {{ width: 10px; height: 10px }} plaintext + div {{ background-image: url(http://test/renamed.png) }}</style>\
                 <link rel=stylesheet href=\"http://test/a.css\" data-pyro-sheet=\"k\"{marker}>\
                 <pre data-pyro-tag=\"plaintext\"{marker}>\nx</pre><div></div><span></span>"
            ),
            base_url: "http://test/".into(),
            width: 40,
            height: 40,
            device_scale_factor: 1.0,
            sheets: [("k".to_owned(), "span { display: block; width: 10px; height: 10px; background-image: url(http://test/sheet.png) }".to_owned())]
                .into_iter()
                .collect(),
            frames: Arc::new(FrameTableData { nonce: "0123456789abcdef0123456789abcdef".into(), entries: Default::default() }),
            ..Default::default()
        };
        Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap();
        let asked = seen.0.lock().unwrap().clone();
        asked
    };
    let marked = render(" data-pyro-n=\"0123456789abcdef0123456789abcdef\"");
    assert!(marked.iter().any(|u| u == "http://test/renamed.png"), "{marked:?}");
    assert!(marked.iter().any(|u| u == "http://test/sheet.png"), "{marked:?}");
    for forged in ["", " data-pyro-n=\"0123456789abcdef0123456789abcdee\""] {
        let asked = render(forged);
        assert!(!asked.iter().any(|u| u == "http://test/renamed.png" || u == "http://test/sheet.png"), "{forged}: {asked:?}");
    }
}
