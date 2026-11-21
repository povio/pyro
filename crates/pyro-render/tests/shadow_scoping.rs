//! Shadow trees in the render tree (CSS Scoping 1): pyro-dom gives the flat tree, with shadow
//! hosts and slots of assigned nodes marked, and blitz-dom scopes each shadow tree's sheets to
//! it: `:host`, `::slotted()`, `::part()`, slots' own boxes, inheritance through the flat tree,
//! and directionality (`:dir()`) through the node trees.
use std::{cell::RefCell, rc::Rc, sync::Arc};

use pyro_dom::{
    parse::{parse_html_unsafe, DomRef},
    snapshot::{render_key, RenderHtml, RenderNodeKind},
};
use pyro_render::*;

struct NoFetch;
impl Fetcher for NoFetch {
    fn fetch(&self, _: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(Err("offline".into()));
    }
}

const STYLE: &str = "<!doctype html><style>body{margin:0;font:20px/40px monospace}</style>";

type Styled = Vec<(LayoutBox, style::servo_arc::Arc<style::properties::ComputedValues>)>;

/// Renders `body` and returns, for each id, the element's box and computed style.
fn styled(body: &str, ids: &[&str]) -> Styled {
    restyled(body, ids, |_, _| {})
}

/// As [`styled`], after rendering once and then running `change` on the DOM (the page's
/// document re-rendered from its new tree, as a live page is).
fn restyled(body: &str, ids: &[&str], change: impl FnOnce(&mut pyro_dom::Dom, pyro_dom::NodeId)) -> Styled {
    let dom: DomRef = Rc::new(RefCell::new(pyro_dom::Dom::new()));
    // Declarative shadow roots (`<template shadowrootmode>`), as `Document.parseHTMLUnsafe`.
    let doc = parse_html_unsafe(&dom, &format!("{STYLE}{body}"));
    let mut renderer = Renderer::new();
    sync(&dom.borrow(), doc, &mut renderer);
    change(&mut dom.borrow_mut(), doc);
    let d = dom.borrow();
    sync(&d, doc, &mut renderer);
    ids.iter()
        .map(|id| {
            let key = render_key(find(&d, doc, id).unwrap_or_else(|| panic!("no element #{id}")));
            (renderer.element_box(1, key).unwrap(), renderer.element_style(1, key).unwrap())
        })
        .collect()
}

/// Lays out the document's render tree as page 1 of `renderer`.
fn sync(d: &pyro_dom::Dom, doc: pyro_dom::NodeId, renderer: &mut Renderer) {
    let tree = d
        .render_tree(doc, &RenderHtml { style_text: &|_| None, stylesheet_href: &|_| None, mark: None, adopted: None })
        .unwrap();
    // (The document's nonce, which its engine state is named with, as the frame table carries it.)
    let nonce = d.frame_table(doc).nonce;
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
        width: 600,
        height: 400,
        device_scale_factor: 1.0,
        live: Some(Live { page: 1, tree: Arc::new(tree), clip: None, interaction: None }),
        frames: Arc::new(pyro_render::FrameTableData { nonce: nonce.clone(), entries: Default::default() }),
        ..Default::default()
    };
    renderer.sync_layout(&snap, &(Arc::new(NoFetch) as Arc<dyn Fetcher>)).unwrap();
}

/// The element with id `id`, in the document or in any shadow tree (tree order, shadow trees
/// before their host's children).
fn find(d: &pyro_dom::Dom, node: pyro_dom::NodeId, id: &str) -> Option<pyro_dom::NodeId> {
    if d.get_attribute(node, "id").ok().flatten() == Some(id) {
        return Some(node);
    }
    if let Some(root) = d.shadow_root(node).ok().flatten() {
        if let Some(found) = find(d, root, id) {
            return Some(found);
        }
    }
    let mut child = d.first_child(node).ok().flatten();
    while let Some(c) = child {
        if let Some(found) = find(d, c, id) {
            return Some(found);
        }
        child = d.next_sibling(c).ok().flatten();
    }
    None
}

/// The element's background colour as `r,g,b`.
fn background(style: &style::properties::ComputedValues) -> String {
    let c = style.resolve_color(&style.get_background().background_color);
    let s = c.to_color_space(style::color::ColorSpace::Srgb);
    let [r, g, b] = s.raw_components()[..3].try_into().map(|c: [f32; 3]| c.map(|v| (v * 255.0).round() as u8)).unwrap();
    format!("{r},{g},{b}")
}

fn rtl(style: &style::properties::ComputedValues) -> bool {
    style.get_inherited_box().direction == style::computed_values::direction::T::Rtl
}

const GREEN: &str = "0,128,0";
const RED: &str = "255,0,0";

#[test]
fn shadow_styles_apply_to_their_tree_only() {
    let r = styled(
        "<style>div{background:green} #h > p{background:green} my-host{display:block}</style>
        <my-host id=h><template shadowrootmode=open><style>div{background:red} :host{background:green} span{background:green}</style><div id=inner></div><span id=s></span><slot></slot></template><p id=light>x</p></my-host>
        <div id=outer></div>",
        &["h", "inner", "s", "light", "outer"],
    );
    let bg: Vec<_> = r.iter().map(|(_, s)| background(s)).collect();
    // `:host` styles the host; the shadow tree's `div` rule is its own, the document's does
    // not reach in; the slotted `<p>` is the host's child for the document's `#h > p`.
    assert_eq!(bg, vec![GREEN, RED, GREEN, GREEN, GREEN], "{bg:?}");
}

#[test]
fn slotted_rules_and_slot_boxes() {
    let r = styled(
        "<my-host id=h style='display:block'><template shadowrootmode=open><style>::slotted(p){background:green} slot{display:block;width:100px;padding:10px;color:rgb(0,128,0)}</style><slot id=slot></slot></template><p id=p style='margin:0'>x</p><span id=t>y</span></my-host>",
        &["slot", "p", "t"],
    );
    // The slot has its own box (`display: block` overrides the UA's `contents`) holding its
    // assigned nodes, which inherit from it.
    assert_eq!((r[0].0.width, r[1].0.x), (120.0, 10.0));
    assert_eq!(background(&r[1].1), GREEN);
    assert_eq!(r[2].1.get_inherited_text().color.clone().into_srgb_legacy().raw_components()[1], 128.0 / 255.0);
}

#[test]
fn has_and_not_on_the_featureless_host() {
    let r = styled(
        "<div id=a><template shadowrootmode=open><style>:host:has(.inner){background:green} :host{width:10px;height:10px}</style><b class=inner></b></template></div>
        <div id=b><template shadowrootmode=open><style>:has(.inner){background:red} :host{background:green}</style><b class=inner></b></template></div>
        <div id=c><template shadowrootmode=open><style>:host{background:red} :not(div, :not(:host)){background:green}</style></template></div>
        <div id=d><template shadowrootmode=open><style>:host{background:green} :not(div){background:red}</style></template></div>
        <div id=e><template shadowrootmode=open><style>:host{background:green} :host:has(~ .after){background:red}</style><b class=inner></b></template></div><p class=after></p>",
        &["a", "b", "c", "d", "e"],
    );
    let bg: Vec<_> = r.iter().map(|(_, s)| background(s)).collect();
    // `:has()` on the featureless host looks into its shadow tree only: not at its siblings (#e).
    assert_eq!(bg, vec![GREEN, GREEN, GREEN, GREEN, GREEN], "{bg:?}");
}

#[test]
fn parts_and_exported_parts() {
    let r = styled(
        "<style>#h::part(label){background:green} #h::part(outer){background:green}</style>
        <div id=h><template shadowrootmode=open><b id=l part=label></b><div id=inner exportparts='label: outer'><template shadowrootmode=open><i id=deep part=label></i></template></div></template></div>",
        &["l", "deep"],
    );
    let bg: Vec<_> = r.iter().map(|(_, s)| background(s)).collect();
    assert_eq!(bg, vec![GREEN, GREEN], "{bg:?}");
}

#[test]
fn linguistic_selectors_match_the_part_not_its_host() {
    let result = styled(
        "<style>#host::part(label){background:red} #host::part(label):lang(fr):dir(rtl){background:green}</style>
        <div id=host lang=en dir=ltr><template shadowrootmode=open>
        <b id=translated part=label lang=fr dir=rtl>x</b><b id=inherited part=label>x</b></template></div>",
        &["translated", "inherited"],
    );
    assert_eq!(background(&result[0].1), GREEN);
    assert_eq!(background(&result[1].1), RED);
}

#[test]
fn directionality_follows_the_node_tree_and_dir_auto_its_text() {
    let r = styled(
        "<style>p:dir(ltr){background:green} p:dir(rtl){background:red}</style>
        <div id=h><template shadowrootmode=open><slot dir=rtl></slot></template><p id=p>x</p></div>
        <p id=auto dir=auto>\u{05D0}bc</p><p id=empty dir=auto>123</p><div dir=rtl><bdi id=bdi>abc</bdi></div>",
        &["p", "auto", "empty", "bdi"],
    );
    // The slotted `<p>` inherits `direction` from its slot (the flat tree), but its
    // directionality is its host's (the node tree): `:dir(ltr)`.
    assert!(rtl(&r[0].1));
    assert_eq!(background(&r[0].1), GREEN);
    assert!(rtl(&r[1].1), "dir=auto with Hebrew first");
    assert!(!rtl(&r[2].1), "dir=auto without a strong character is ltr");
    assert!(!rtl(&r[3].1), "a <bdi> takes its own text's direction");
}

#[test]
fn a_slot_with_fallback_in_a_shadow_tree_and_one_in_the_document_render() {
    let r = styled(
        "<style>.red{background:red} .green{background:green} div{width:100px;height:50px} slot{border:solid 10px red}</style>
        <my-host id=h style='display:block;width:100px;height:50px'><template shadowrootmode=open><slot style='border:solid 10px red'><div id=a style='width:100%;height:100%;background:green'></div></slot></template></my-host>
        <div class=red><slot><div id=b class=green></div></slot></div>",
        &["a", "b"],
    );
    assert_eq!((r[0].0.width, r[0].0.height), (100.0, 50.0));
    assert_eq!((r[1].0.width, r[1].0.height), (100.0, 50.0));
}

#[test]
fn host_has_follows_changes_in_the_shadow_tree() {
    let body = "<div id=h><template shadowrootmode=open><style>:host:has(.x){background:green} :host:has(.x) b{background:green}</style><b id=b></b><i id=i></i></template></div>";
    let on = restyled(body, &["h", "b"], |d, doc| {
        let i = find(d, doc, "i").unwrap();
        d.set_attribute(i, "class", "x").unwrap();
    });
    assert_eq!((background(&on[0].1), background(&on[1].1)), (GREEN.into(), GREEN.into()));
    let body = body.replace("<i id=i>", "<i id=i class=x>");
    let off = restyled(&body, &["h", "b"], |d, doc| {
        let i = find(d, doc, "i").unwrap();
        d.remove_attribute(i, "class").unwrap();
    });
    assert_eq!((background(&off[0].1), background(&off[1].1)), ("0,0,0".into(), "0,0,0".into()));
}

#[test]
fn reassigned_and_reslotted_nodes_restyle() {
    // A slot that gains a name stops taking the default slot's nodes; the other slot takes them
    // and its style is theirs to inherit (`reslot-text-inheritance`).
    let r = restyled(
        "<p id=h><template shadowrootmode=open><slot id=g name=other style='color:green'></slot><slot id=r style='color:red'></slot></template><span id=t>text</span></p>",
        &["t"],
        |d, doc| {
            let g = find(d, doc, "g").unwrap();
            d.remove_attribute(g, "name").unwrap();
        },
    );
    assert_eq!(r[0].1.get_inherited_text().color.clone().into_srgb_legacy().raw_components()[1], 128.0 / 255.0);
}

#[test]
fn slotted_pseudo_elements_generate_boxes() {
    // `::slotted(span)::before` gives the slotted span a `::before` (20px monospace: 12px a letter).
    let r = styled(
        "<div id=h><template shadowrootmode=open><style>::slotted(span)::before{content:'abcd'}</style><slot></slot></template><span id=s>x</span></div>",
        &["s"],
    );
    assert!(r[0].0.width > 50.0, "{}", r[0].0.width);
}

#[test]
fn custom_elements_not_defined_yet_do_not_match_defined() {
    // `:not(:defined)` hides components until they are upgraded (a common way to avoid a flash of
    // unstyled content); every other element is `:defined`.
    let r = styled(
        "<style>:not(:defined){background:red} div:defined{background:green}</style><x-card id=c>x</x-card><div id=d>y</div>",
        &["c", "d"],
    );
    assert_eq!((background(&r[0].1), background(&r[1].1)), (RED.into(), GREEN.into()));
}

#[test]
fn text_of_display_contents_elements_takes_their_styles() {
    // The words of a `display: contents` span are laid out in its font size, as a slot's assigned
    // text is (CSS Display: the element's box is replaced by its contents, which inherit from it).
    let r = styled(
        "<div id=a style='line-height:1.5'><b>m</b><span style='display:contents;font-size:40px'>mmmm</span></div>
        <div id=b style='line-height:1.5'><span style='display:contents;font-size:40px'>mmmm</span></div>
        <p id=h style='line-height:1.5;margin:0'><template shadowrootmode=open><slot style='font-size:40px'></slot></template>mmmm</p>",
        &["a", "b", "h"],
    );
    // Lines of 40px text are 60px tall (20px text's 30px), whether the text is in an inline formatting
    // context (#a), hoisted into an anonymous block (#b), or a slot's (#h).
    let heights: Vec<_> = r.iter().map(|x| x.0.height).collect();
    assert!(heights.iter().all(|&h| h >= 55.0), "{heights:?}");
}

#[test]
fn host_context_matches_the_host_and_its_ancestors() {
    let r = styled(
        "<div class=theme><div id=a><template shadowrootmode=open><style>:host-context(.theme){background:green} :host{background:red}</style></template></div></div>
        <div id=b><template shadowrootmode=open><style>:host-context(#b){background:green}</style></template></div>
        <div id=c><template shadowrootmode=open><style>:host{background:green} :host-context(.theme){background:red}</style></template></div>",
        &["a", "b", "c"],
    );
    let bg: Vec<_> = r.iter().map(|(_, s)| background(s)).collect();
    // `:host-context(X)` has `:host(X)`'s specificity: it wins over `:host` (#a).
    assert_eq!(bg, vec![GREEN, GREEN, GREEN], "{bg:?}");
}

#[test]
fn a_shadow_host_matches_focus_while_its_shadow_tree_holds_the_focused_element() {
    let dom: DomRef = Rc::new(RefCell::new(pyro_dom::Dom::new()));
    let doc = parse_html_unsafe(
        &dom,
        &format!(
            "{STYLE}<style>#h{{background:red}} #h:focus{{background:green}}</style>\
             <div id=h><template shadowrootmode=open><div id=inner tabindex=0></div></template></div>"
        ),
    );
    let mut renderer = Renderer::new();
    let d = dom.borrow();
    sync(&d, doc, &mut renderer);
    let host = render_key(find(&d, doc, "h").unwrap());
    let inner = render_key(find(&d, doc, "inner").unwrap());
    renderer.set_interaction(1, None, Some(inner), false);
    assert_eq!(background(&renderer.element_style(1, host).unwrap()), GREEN);
    renderer.set_interaction(1, None, None, false);
    assert_eq!(background(&renderer.element_style(1, host).unwrap()), RED);
}

#[test]
fn part_rules_of_an_outer_shadow_tree() {
    // `display-el::part(icon)` in the shadow tree that holds `display-el` styles its part.
    let r = styled(
        "<wrapper-el><template shadowrootmode=open><style>display-el::part(icon){background:green}</style>\
         <display-el><template shadowrootmode=open><style>div{width:16px;height:16px;background:red}</style>\
         <div id=icon part=icon></div></template></display-el></template></wrapper-el>",
        &["icon"],
    );
    assert_eq!(background(&r[0].1), GREEN);
}

#[test]
fn a_manually_assigning_slot_renders_its_assigned_nodes() {
    // Built from a tree after `assign()`: the slot holds the nodes assigned to it.
    let r = restyled(
        "<div id=h><template shadowrootmode=open shadowrootslotassignment=manual><slot id=s style='color:green'></slot></template>\
         <span id=a>a</span><span id=b>b</span></div>",
        &["a"],
        |d, doc| {
            let s = find(d, doc, "s").unwrap();
            let a = find(d, doc, "a").unwrap();
            d.slot_assign(s, &[a]).unwrap();
        },
    );
    let green = |s: &style::properties::ComputedValues| {
        s.get_inherited_text().color.clone().into_srgb_legacy().raw_components()[1]
    };
    assert_eq!(green(&r[0].1), 128.0 / 255.0, "the assigned span inherits from the slot");
}

/// The page `body` painted (from markup, as blitz-html parses it).
fn paint(body: &str) -> RgbaImage {
    let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
    let snapshot = Snapshot {
        html: format!("{STYLE}{body}"),
        base_url: "http://test/".into(),
        width: 400,
        height: 200,
        device_scale_factor: 1.0,
        ..Default::default()
    };
    Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0
}

#[test]
fn list_items_in_display_contents_children_are_numbered_on() {
    // A slot (or any `display: contents` element) between a list and its items does not restart
    // their numbering (WPT css-lists `counter-list-item-slot-order`).
    let flat = paint("<ol><li>x</li><li>y</li><li>z</li></ol>");
    let wrapped = paint(
        "<ol><div style='display:contents'><li>x</li></div><div style='display:contents'><li>y</li><li>z</li></div></ol>",
    );
    assert!(flat.data == wrapped.data, "the markers read 1. 2. 3. either way");
}

/// `:has-slotted(<selector>)` follows changes of the slot's assigned elements: a class set on and
/// removed from an assigned node, and a node assigned to the slot by its `slot` attribute.
#[test]
fn has_slotted_follows_changes_of_assigned_nodes() {
    let body = "<div id=h><template shadowrootmode=open><style>slot{display:block;background:red} slot:has-slotted(.on){background:green}</style><slot id=s></slot></template><span id=a>a</span></div>";
    let on = restyled(body, &["s"], |d, doc| {
        let a = find(d, doc, "a").unwrap();
        d.set_attribute(a, "class", "on").unwrap();
    });
    assert_eq!(background(&on[0].1), GREEN, "class set");
    let body_on = body.replace("<span id=a>", "<span id=a class=on>");
    let off = restyled(&body_on, &["s"], |d, doc| {
        let a = find(d, doc, "a").unwrap();
        d.remove_attribute(a, "class").unwrap();
    });
    assert_eq!(background(&off[0].1), RED, "class removed");
    let body_elsewhere = body.replace("<span id=a>", "<span id=a class=on slot=other>");
    let assigned = restyled(&body_elsewhere, &["s"], |d, doc| {
        let a = find(d, doc, "a").unwrap();
        d.remove_attribute(a, "slot").unwrap();
    });
    assert_eq!(background(&assigned[0].1), GREEN, "assigned by its slot attribute");
}
