//! Animated values the page samples into a persistent document's cascade
//! (`Snapshot::animations`, blitz-dom's `animation` module): keyframes resolve
//! against the element's base style and interpolate as Web Animations
//! samples a keyframe effect; a frame without the element drops its values.
use std::{cell::RefCell, rc::Rc, sync::Arc};

use pyro_dom::parse::{parse_document, DomRef};
use pyro_dom::snapshot::{render_key, RenderHtml, RenderNodeKind};
use pyro_render::animation::{Composite, Effect, ElementEffects, Keyframes, TextKeyframe, TextKeyframes};
use pyro_render::*;

struct NoFetch;
impl Fetcher for NoFetch {
    fn fetch(&self, _: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(Err("offline".into()));
    }
}

/// The page's live snapshot and the key of `#target`.
fn page(html: &str) -> (Snapshot, u64) {
    let dom: DomRef = Rc::new(RefCell::new(pyro_dom::Dom::new()));
    let doc = parse_document(&dom, "http://test/", html);
    let d = dom.borrow();
    let key = render_key(d.get_element_by_id(doc, "target").unwrap().unwrap());
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
        width: 200,
        height: 100,
        device_scale_factor: 1.0,
        live: Some(Live { page: 1, tree: Arc::new(tree), clip: None, interaction: None }),
        ..Default::default()
    };
    (snap, key)
}

fn render(renderer: &mut Renderer, snap: &Snapshot, frame: AnimationFrame) -> RgbaImage {
    let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
    let snap = Snapshot { animations: Some(Arc::new(frame)), ..snap.clone() };
    renderer.render(&snap, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0
}

fn pixel(img: &RgbaImage, x: u32, y: u32) -> [u8; 3] {
    let i = (y as usize * img.width as usize + x as usize) * 4;
    [img.data[i], img.data[i + 1], img.data[i + 2]]
}

fn assert_near(got: [u8; 3], want: [u8; 3], what: &str) {
    assert!(got.iter().zip(want).all(|(&g, w)| g.abs_diff(w) <= 1), "{what}: {got:?}, want {want:?}");
}

fn css(name: &str, easing: Option<&str>, progress: f64) -> Effect {
    Effect {
        keyframes: Keyframes::Css { name: name.into(), easing: easing.map(Arc::from), composite: Composite::Replace },
        progress,
        before: false,
        easing: None,
        composite: Composite::Replace,
    }
}

fn text(frames: &[(f64, &str)], progress: f64, easing: Option<&str>) -> Effect {
    Effect {
        keyframes: Keyframes::Text(Arc::new(TextKeyframes {
            frames: frames
                .iter()
                .map(|(offset, declarations)| TextKeyframe {
                    offset: *offset,
                    easing: None,
                    composite: None,
                    declarations: (*declarations).into(),
                })
                .collect(),
        })),
        progress,
        before: false,
        easing: easing.map(Arc::from),
        composite: Composite::Replace,
    }
}

fn frame(key: u64, transitions: Vec<Effect>, animations: Vec<Effect>) -> AnimationFrame {
    vec![(key, ElementEffects { transitions, animations, current: Vec::new() })]
}

const BOX: &str = "<!doctype html><style>body{margin:0;background:#fff} #target{width:100px;height:100px;background-color:rgb(0,200,0)}\
    @keyframes both{from{background-color:rgb(0,200,0)}to{background-color:rgb(200,0,0)}}\
    @keyframes to-red{to{background-color:rgb(200,0,0)}}\
    @keyframes stepped{0%{background-color:rgb(0,0,0);animation-timing-function:steps(2,jump-end)}100%{background-color:rgb(200,200,200)}}</style>\
    <div id=target></div>";

/// A CSS animation's `@keyframes` interpolate at the progress the page gives
/// (linear here), and an empty frame puts the base value back.
#[test]
fn a_css_animation_is_sampled_at_its_progress_and_dropped_with_it() {
    let (snap, key) = page(BOX);
    let mut r = Renderer::new();
    let img = render(&mut r, &snap, frame(key, vec![], vec![css("both", Some("linear"), 0.5)]));
    assert_near(pixel(&img, 50, 50), [100, 100, 0], "halfway");
    let img = render(&mut r, &snap, frame(key, vec![], vec![css("both", Some("linear"), 0.25)]));
    assert_near(pixel(&img, 50, 50), [50, 150, 0], "a quarter of the way, the same document");
    let img = render(&mut r, &snap, Vec::new());
    assert_near(pixel(&img, 50, 50), [0, 200, 0], "no animation: the base value");
}

/// A missing 0% keyframe takes the element's base value (not the animated
/// one it had last), and an unknown `@keyframes` name animates nothing.
#[test]
fn an_implicit_keyframe_is_the_base_value() {
    let (snap, key) = page(BOX);
    let mut r = Renderer::new();
    for progress in [0.9, 0.5] {
        let img = render(&mut r, &snap, frame(key, vec![], vec![css("to-red", Some("linear"), progress)]));
        let want = if progress == 0.5 { [100, 100, 0] } else { [180, 20, 0] };
        assert_near(pixel(&img, 50, 50), want, &format!("at {progress}"));
    }
    let img = render(&mut r, &snap, frame(key, vec![], vec![css("missing", None, 0.5)]));
    assert_near(pixel(&img, 50, 50), [0, 200, 0], "unknown keyframes");
}

/// A keyframe's own `animation-timing-function` eases its interval; the
/// default of a CSS animation is `ease`.
#[test]
fn keyframe_easing_applies_per_interval() {
    let (snap, key) = page(BOX);
    let mut r = Renderer::new();
    let img = render(&mut r, &snap, frame(key, vec![], vec![css("stepped", None, 0.4)]));
    assert_near(pixel(&img, 50, 50), [0, 0, 0], "first step");
    let img = render(&mut r, &snap, frame(key, vec![], vec![css("stepped", None, 0.6)]));
    assert_near(pixel(&img, 50, 50), [100, 100, 100], "second step");
    // `ease` at 0.5 is 0.8024.
    let img = render(&mut r, &snap, frame(key, vec![], vec![css("both", None, 0.5)]));
    assert_near(pixel(&img, 50, 50), [160, 40, 0], "ease");
}

/// Keyframes as text (script animations, transitions) with the effect's
/// easing; the transitions level wins over the animations level.
#[test]
fn text_keyframes_with_effect_easing_and_the_transitions_level() {
    let (snap, key) = page(BOX);
    let mut r = Renderer::new();
    let fade = text(&[(0.0, "background-color: rgb(0, 0, 200)"), (1.0, "background-color: rgb(200, 0, 0)")], 0.5, None);
    let img = render(&mut r, &snap, frame(key, vec![], vec![fade.clone()]));
    assert_near(pixel(&img, 50, 50), [100, 0, 100], "script keyframes");
    let stepped = Effect { easing: Some(Arc::from("steps(1, start)")), ..fade.clone() };
    let img = render(&mut r, &snap, frame(key, vec![], vec![stepped]));
    assert_near(pixel(&img, 50, 50), [200, 0, 0], "effect easing");
    let transition =
        text(&[(0.0, "background-color: rgb(0, 0, 0)"), (1.0, "background-color: rgb(200, 200, 200)")], 0.5, None);
    let img = render(&mut r, &snap, frame(key, vec![transition], vec![fade]));
    assert_near(pixel(&img, 50, 50), [100, 100, 100], "transition over animation");
}

/// Layout-affecting values move boxes: an animated `width` relays out.
#[test]
fn an_animated_width_lays_out() {
    let (snap, key) = page(BOX);
    let mut r = Renderer::new();
    let grow = text(&[(0.0, "width: 100px"), (1.0, "width: 200px")], 0.5, None);
    let img = render(&mut r, &snap, frame(key, vec![], vec![grow]));
    assert_near(pixel(&img, 140, 50), [0, 200, 0], "inside the wider box");
    assert_near(pixel(&img, 160, 50), [255, 255, 255], "past it");
}

/// Layout queries: a style-only sync (a computed style read) samples an
/// animation the document did not have before, as a full one does.
#[test]
fn a_style_only_sync_samples_a_new_animation() {
    let (snap, key) = page(BOX);
    let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
    let mut r = Renderer::new();
    r.sync_layout(&snap, &fetcher).unwrap();
    let fade = text(&[(0.0, "opacity: 0"), (1.0, "opacity: 1")], 0.5, None);
    r.set_animations(1, Arc::new(frame(key, vec![], vec![fade])));
    r.sync_delta(1, &[], (200, 100, 1.0, false, false), true).unwrap();
    let opacity = r.element_style(1, key).unwrap().get_effects().opacity;
    assert_eq!(opacity, 0.5, "style only");
    r.set_animations(1, Arc::new(Vec::new()));
    r.sync_delta(1, &[], (200, 100, 1.0, false, false), false).unwrap();
    assert_eq!(r.element_style(1, key).unwrap().get_effects().opacity, 1.0, "dropped");
}

/// An opacity animation in effect makes its element a backdrop root, as
/// `will-change: opacity` would, at the end of a filling animation too, where
/// the opacity is 1 again (WPT backdrop-filter-backdrop-root-animation-in-effect):
/// the child's backdrop is what the element painted before it (nothing), not
/// the page.
#[test]
fn an_opacity_animation_in_effect_makes_a_backdrop_root() {
    const PAGE: &str = "<!doctype html><style>body{margin:0;background:#0ff} #target{width:100px;height:100px}\
        #child{width:100px;height:100px;backdrop-filter:invert(1)}</style><div id=target><div id=child></div></div>";
    let (snap, key) = page(PAGE);
    let mut r = Renderer::new();
    let img = render(&mut r, &snap, Vec::new());
    assert_near(pixel(&img, 50, 50), [255, 0, 0], "no animation: the page, inverted");
    let fade = text(&[(0.0, "opacity: 0"), (1.0, "opacity: 1")], 1.0, None);
    let img = render(&mut r, &snap, frame(key, vec![], vec![fade]));
    assert_near(pixel(&img, 50, 50), [0, 255, 255], "in effect at its end: a backdrop root");
}

/// An animation of a property that makes a stacking context makes its element one while it is
/// current, in its delay too, where it gives no value (Web Animations' `will-change` side effect;
/// WPT `individual-transform/stacking-context-001`): the element then paints with the positioned
/// boxes, over the earlier fixed box. Without the animation it is in-flow content, under it.
#[test]
fn an_animation_in_its_delay_makes_a_stacking_context() {
    const PAGE: &str = "<!doctype html><style>body{margin:0;background:#fff}\
        #back{position:fixed;top:50px;width:100px;height:100px;background:rgb(0,128,0)}\
        #target{width:100px;height:100px;background:rgb(0,0,255)}\
        @keyframes scale{from,to{scale:1}} @keyframes color{to{background:red}}</style>\
        <div id=back></div><div id=target></div>";
    let (snap, key) = page(PAGE);
    let mut r = Renderer::new();
    let img = render(&mut r, &snap, Vec::new());
    assert_near(pixel(&img, 50, 75), [0, 128, 0], "no animation: under the fixed box");
    let current = |name: &str| {
        vec![(key, ElementEffects { transitions: vec![], animations: vec![], current: vec![css(name, None, 0.0)] })]
    };
    let img = render(&mut r, &snap, current("scale"));
    assert_near(pixel(&img, 50, 75), [0, 0, 255], "a `scale` animation in its delay: over it");
    assert_near(pixel(&img, 50, 25), [0, 0, 255], "and no value: its own colour");
    let img = render(&mut r, &snap, current("color"));
    assert_near(pixel(&img, 50, 75), [0, 128, 0], "a colour animation in its delay: under it");
    assert_near(pixel(&img, 50, 25), [0, 0, 255], "with no value");
    let img = render(&mut r, &snap, Vec::new());
    assert_near(pixel(&img, 50, 75), [0, 128, 0], "the animation gone: under it again");
}
