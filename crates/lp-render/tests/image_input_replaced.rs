//! `<input type=image>` is replaced content laid out like `<img>`: a box at
//! the image's natural size (recorded by the embedder, carried to Blitz as
//! the render tree's `natural=W,H` state), sized by the width/height
//! attributes and CSS like an image, without the text-input border and
//! padding. Expected values are Chromium's (8px body margin, 200x200 image).
use std::{cell::RefCell, rc::Rc, sync::Arc};

use lp_dom::{
    parse::{parse_document, DomRef},
    snapshot::{render_key, RenderHtml, RenderNodeKind},
};
use lp_render::*;

struct NoFetch;
impl Fetcher for NoFetch {
    fn fetch(&self, _: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(Err("offline".into()));
    }
}

/// The border boxes of `ids`, with every `[data-natural="W,H"]` element's
/// natural size recorded as a loaded image's would be.
fn boxes(html: &str, ids: &[&str]) -> Vec<LayoutBox> {
    let dom: DomRef = Rc::new(RefCell::new(lp_dom::Dom::new()));
    let doc = parse_document(&dom, "http://test/", html);
    let mut d = dom.borrow_mut();
    let natural = d.query_selector_all(doc, &lp_dom::select::parse_selectors("[data-natural]").unwrap()).unwrap();
    for el in natural {
        let (w, h) = {
            let v = d.get_attribute(el, "data-natural").unwrap().unwrap();
            let (w, h) = v.split_once(',').unwrap();
            (w.parse().unwrap(), h.parse().unwrap())
        };
        d.set_natural_size(el, Some((w, h)));
    }
    let keys: Vec<_> = ids.iter().map(|id| render_key(d.get_element_by_id(doc, id).unwrap().unwrap())).collect();
    let tree = d
        .render_tree(doc, &RenderHtml { style_text: &|_| None, stylesheet_href: &|_| None, mark: None, adopted: None })
        .unwrap();
    let tree = Tree {
        nodes: tree
            .nodes
            .into_iter()
            .map(|node| TreeNode {
                key: node.key,
                parent: node.parent,
                kind: match node.kind {
                    RenderNodeKind::Element { name, attrs } => TreeKind::Element { name, attrs },
                    RenderNodeKind::Text(text) => TreeKind::Text(text),
                },
            })
            .collect(),
    };
    let snap = Snapshot {
        base_url: "http://test/".into(),
        width: 800,
        height: 600,
        device_scale_factor: 1.0,
        live: Some(Live { page: 1, tree: Arc::new(tree), clip: None, interaction: None }),
        ..Default::default()
    };
    let mut renderer = Renderer::new();
    renderer.sync_layout(&snap, &(Arc::new(NoFetch) as Arc<dyn Fetcher>)).unwrap();
    keys.into_iter().map(|key| renderer.element_box(1, key).unwrap()).collect()
}

fn rect(b: &LayoutBox) -> [f64; 4] {
    [b.x, b.y, b.width, b.height]
}

#[test]
fn image_input_takes_its_images_natural_size_like_img() {
    let b = boxes(
        r#"<!doctype html><body>
          <div><input id=input type=image src=i.svg data-natural="200,200"></div>
          <div><img id=img src=i.svg data-natural="200,200"></div>
          <div><input id=text type=text></div>"#,
        &["input", "img", "text"],
    );
    assert_eq!(rect(&b[0])[2..], [200.0, 200.0], "image input: {:?}", b[0]);
    assert_eq!(rect(&b[0])[..2], [8.0, 8.0]);
    assert_eq!(rect(&b[1])[2..], [200.0, 200.0], "img: {:?}", b[1]);
    // Chromium's UA sheet: no text-input border, padding or background.
    assert_eq!(b[0].border, [0.0; 4]);
    assert_eq!(b[0].padding, [0.0; 4]);
    // A text input keeps its text-input box.
    assert_eq!(b[2].border, [2.0; 4], "text input: {:?}", b[2]);
    assert!(b[2].width > 100.0 && b[2].height < 30.0, "text input: {:?}", b[2]);
}

#[test]
fn width_height_attributes_and_css_size_an_image_input_like_img() {
    let b = boxes(
        r#"<!doctype html><body style="margin:0">
          <input id=w50 type=image src=i.svg width=50 data-natural="200,100" style="display:block">
          <input id=h20 type=image src=i.svg style="display:block;height:20px" data-natural="200,100">
          <input id=both type=image src=i.svg width=30 height=70 data-natural="200,100" style="display:block">
          <input id=upper type=IMAGE src=i.svg data-natural="200,100" style="display:block">
          <input id=maxw type=image src=i.svg data-natural="200,100" style="display:block;max-width:100px">"#,
        &["w50", "h20", "both", "upper", "maxw"],
    );
    // One dimension follows the natural aspect ratio (2:1).
    assert_eq!(rect(&b[0]), [0.0, 0.0, 50.0, 25.0], "width=50: {:?}", b[0]);
    assert_eq!(rect(&b[1]), [0.0, 25.0, 40.0, 20.0], "height:20px: {:?}", b[1]);
    assert_eq!(rect(&b[2]), [0.0, 45.0, 30.0, 70.0], "width=30 height=70: {:?}", b[2]);
    // The type keyword is case-insensitive.
    assert_eq!(rect(&b[3]), [0.0, 115.0, 200.0, 100.0], "type=IMAGE: {:?}", b[3]);
    assert_eq!(rect(&b[4]), [0.0, 215.0, 100.0, 50.0], "max-width: {:?}", b[4]);
}

#[test]
fn unloaded_image_input_is_an_empty_box() {
    // No natural size (not loaded, failed, or no src): a 0x0 box, as an
    // unloaded `<img>` without alt text is.
    let b = boxes(
        r#"<!doctype html><body style="margin:0">
          <input id=nosrc type=image style="display:block">
          <input id=unloaded type=image src=missing.svg style="display:block">
          <img id=img src=missing.svg style="display:block">"#,
        &["nosrc", "unloaded", "img"],
    );
    for (i, id) in ["nosrc", "unloaded", "img"].iter().enumerate() {
        assert_eq!(rect(&b[i])[2..], [0.0, 0.0], "#{id}: {:?}", b[i]);
    }
}

/// Blitz fetches an image button's `src` as it does `<img src>`: the box is
/// sized by the decoded image and painted with it (a screenshot shows the
/// image, as Chromium's does), not left blank.
#[test]
fn a_loaded_image_input_paints_its_image() {
    // A 10x10 red PNG served by the fetcher.
    let red = encode_png(&RgbaImage { width: 10, height: 10, data: [255, 0, 0, 255].repeat(100) });
    struct One(Vec<u8>);
    impl Fetcher for One {
        fn fetch(&self, url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
            let body = self.0.clone();
            std::thread::spawn(move || done(if url.ends_with("red.png") { Ok(body) } else { Err("404".into()) }));
        }
    }
    let snap = Snapshot {
        html: r#"<!doctype html><body style="margin:0;background:#00ff00">
            <input id=button type=image src="red.png" style="display:block">
            <input id=sized type=IMAGE src="red.png" width=50 height=50 style="display:block;margin-top:10px">
            <input id=text type=text src="red.png" style="display:block;margin-top:10px">"#
            .into(),
        base_url: "http://test/".into(),
        width: 400,
        height: 300,
        device_scale_factor: 1.0,
        ..Default::default()
    };
    let fetcher: Arc<dyn Fetcher> = Arc::new(One(red));
    let mut renderer = Renderer::new();
    let (img, _, t) = renderer.render(&snap, Area::Viewport, Backend::Cpu, &fetcher).unwrap();
    let pixel = |x: u32, y: u32| -> [u8; 4] {
        let i = (y as usize * img.width as usize + x as usize) * 4;
        img.data[i..i + 4].try_into().unwrap()
    };
    let (red, green) = ([255, 0, 0, 255], [0, 255, 0, 255]);
    // The image at its natural 10x10 size at (0, 0)...
    assert_eq!(pixel(5, 5), red, "natural-size image button painted ({t:?})");
    assert_eq!(pixel(15, 5), green, "natural-size image button is 10px wide ({t:?})");
    assert_eq!(pixel(5, 15), green, "natural-size image button is 10px tall ({t:?})");
    // ...then the width/height-sized one from y=20 to 70.
    assert_eq!(pixel(25, 45), red, "width/height-sized image button painted ({t:?})");
    assert_eq!(pixel(55, 45), green, "sized image button is 50px wide ({t:?})");
    assert_eq!(pixel(25, 75), green, "sized image button ends at 70px ({t:?})");
    // A text input never shows its `src`.
    assert_ne!(pixel(25, 90), red, "text input must not paint an image ({t:?})");
}
