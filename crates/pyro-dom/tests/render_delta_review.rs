//! `render_delta` builds a changed node's op from that node alone (the
//! walk stops at the first node it writes): the op must still equal the
//! fresh tree's node for nested containers changed in one delta, a text
//! and an attribute change on one node, a node removed and re-added in
//! one delta, shadow hosts, textareas and `<select>`s.

use std::cell::RefCell;
use std::rc::Rc;

use pyro_dom::parse::{parse_document, parse_fragment, DomRef};
use pyro_dom::select::parse_selectors;
use pyro_dom::snapshot::{render_key, DeltaOp, RenderHtml, RenderNodeKind, RenderTree, SYNTHETIC_TEXT};
use pyro_dom::*;

fn doc(html: &str) -> (DomRef, NodeId) {
    let dom: DomRef = Rc::new(RefCell::new(Dom::new()));
    let d = parse_document(&dom, "http://test/", html);
    (dom, d)
}

fn find(dom: &Dom, d: NodeId, css: &str) -> NodeId {
    dom.query_selector(d, &parse_selectors(css).unwrap()).unwrap().unwrap()
}

const OPTS: RenderHtml<'static> = RenderHtml { style_text: &|_| None, stylesheet_href: &|_| None, mark: None, adopted: None };

/// Every `Node` op's kind is the fresh tree's node of that key; returns the
/// keys of the `Node` ops and the parents of the `Children` ops.
#[track_caller]
fn node_ops_match_fresh(delta: &[DeltaOp], fresh: &RenderTree) -> (Vec<u64>, Vec<Option<u64>>) {
    let mut nodes = Vec::new();
    let mut children = Vec::new();
    for op in delta {
        match op {
            DeltaOp::Node { key, kind } => {
                let f = fresh.nodes.iter().find(|n| n.key == *key).unwrap_or_else(|| panic!("{key:#x} not rendered"));
                assert_eq!(*kind, f.kind, "op for {key:#x}");
                nodes.push(*key);
            }
            DeltaOp::Children { parent, kids, nodes: inserted } => {
                // The inserted subtrees' nodes equal the fresh tree's nodes.
                for n in inserted {
                    let f = fresh.nodes.iter().find(|x| x.key == n.key).unwrap_or_else(|| panic!("{:#x} not rendered", n.key));
                    assert_eq!(n.kind, f.kind, "inserted {:#x}", n.key);
                }
                // And `kids` are the fresh tree's children of `parent`.
                let want: Vec<u64> = match parent {
                    Some(p) => {
                        let pi = fresh.nodes.iter().position(|n| n.key == *p).unwrap() as u32;
                        fresh.nodes.iter().filter(|n| n.parent == pi).map(|n| n.key).collect()
                    }
                    None => fresh.nodes.iter().filter(|n| n.parent == pyro_dom::snapshot::TOP).map(|n| n.key).collect(),
                };
                assert_eq!(*kids, want, "children of {parent:?}");
                children.push(*parent);
            }
        }
    }
    nodes.sort_unstable();
    (nodes, children)
}

fn sorted(mut keys: Vec<u64>) -> Vec<u64> {
    keys.sort_unstable();
    keys
}

#[test]
fn nested_containers_changed_in_one_delta_each_carry_their_own_node() {
    let (dom, d) = doc("<body><div id=outer class=a><div id=mid><div id=inner><span id=leaf>t</span></div></div></div></body>");
    let (outer, mid, inner, leaf) =
        (find(&dom.borrow(), d, "#outer"), find(&dom.borrow(), d, "#mid"), find(&dom.borrow(), d, "#inner"), find(&dom.borrow(), d, "#leaf"));
    let text = dom.borrow().first_child(leaf).unwrap().unwrap();
    let since = dom.borrow().version();
    {
        let mut dom = dom.borrow_mut();
        // Outer first, then inner, then the leaf's text and attribute, then
        // the middle one: order of mutation must not matter.
        dom.set_attribute(outer, "class", "b").unwrap();
        dom.set_attribute(inner, "style", "color: red").unwrap();
        dom.set_char_data(text, "changed").unwrap();
        dom.set_attribute(leaf, "title", "x").unwrap();
        dom.set_attribute(mid, "data-k", "v").unwrap();
        dom.set_attribute(outer, "id", "outer2").unwrap();
    }
    let fresh = dom.borrow().render_tree(d, &OPTS).unwrap();
    let delta = dom.borrow().render_delta(d, since, &OPTS).expect("incremental");
    let (nodes, children) = node_ops_match_fresh(&delta.ops, &fresh);
    assert!(children.is_empty(), "{children:?}");
    assert_eq!(nodes, sorted(vec![render_key(outer), render_key(mid), render_key(inner), render_key(leaf), render_key(text)]));
}

#[test]
fn a_node_removed_and_re_added_in_one_delta_is_serialized_whole_by_its_parent() {
    let (dom, d) = doc("<body><ul id=list><li id=a>a</li><li id=b><b>b</b><i>i</i></li><li id=c>c</li></ul></body>");
    let (list, a, b, c) = (find(&dom.borrow(), d, "#list"), find(&dom.borrow(), d, "#a"), find(&dom.borrow(), d, "#b"), find(&dom.borrow(), d, "#c"));
    let since = dom.borrow().version();
    {
        let mut dom = dom.borrow_mut();
        // `b` leaves and comes back last, changed meanwhile (an attribute,
        // its child's text, a new grandchild).
        dom.remove_child(list, b).unwrap();
        dom.set_attribute(b, "class", "moved").unwrap();
        let bold = dom.first_child(b).unwrap().unwrap();
        let bt = dom.first_child(bold).unwrap().unwrap();
        dom.set_char_data(bt, "B").unwrap();
        dom.append_child(list, b).unwrap();
        let em = dom.create_element(d, "em").unwrap();
        dom.append_child(b, em).unwrap();
        // And `a` changes in place, `c` is removed and re-inserted where it was.
        dom.set_attribute(a, "class", "same-place").unwrap();
        dom.remove_child(list, c).unwrap();
        dom.insert_before(list, c, Some(b)).unwrap();
    }
    let fresh = dom.borrow().render_tree(d, &OPTS).unwrap();
    let delta = dom.borrow().render_delta(d, since, &OPTS).expect("incremental");
    let (nodes, children) = node_ops_match_fresh(&delta.ops, &fresh);
    assert_eq!(children, [Some(render_key(list))]);
    // Nothing inside the re-inserted subtrees gets an op of its own.
    assert_eq!(nodes, [render_key(a)]);
    let DeltaOp::Children { kids, nodes: inserted, .. } = &delta.ops.iter().find(|op| matches!(op, DeltaOp::Children { .. })).unwrap() else {
        unreachable!()
    };
    assert_eq!(*kids, [render_key(a), render_key(c), render_key(b)]);
    // `b`'s whole subtree (b, b's bold, its text, i, its text, em) and c's.
    assert_eq!(inserted.len(), 6 + 2, "{inserted:?}");
}

#[test]
fn hosts_textareas_selects_and_void_elements_changed_alone() {
    let (dom, d) = doc(concat!(
        "<body><div id=host><span slot=s>light</span></div>",
        "<textarea id=ta rows=2>default</textarea>",
        "<select id=sel><option id=o1>1</option><option id=o2>2</option></select>",
        "<input id=in value=v><img id=img src=x.png><br id=br>",
        "<p id=p>text</p></body>"
    ));
    let (host, ta, sel, o2, input, img, br, p) = (
        find(&dom.borrow(), d, "#host"),
        find(&dom.borrow(), d, "#ta"),
        find(&dom.borrow(), d, "#sel"),
        find(&dom.borrow(), d, "#o2"),
        find(&dom.borrow(), d, "#in"),
        find(&dom.borrow(), d, "#img"),
        find(&dom.borrow(), d, "#br"),
        find(&dom.borrow(), d, "#p"),
    );
    {
        let mut dom = dom.borrow_mut();
        // (No shadow root here: any shadow root makes `render_delta` fall
        // back to a full render, see the test below.)
        dom.set_value(ta, "typed").unwrap();
        dom.set_value(input, "typed in").unwrap();
    }
    // Each mutation alone: which ones fall back to a full render (None),
    // which give ops equal to the fresh tree's nodes.
    {
        type M = Box<dyn Fn(&mut Dom)>;
        let probes: Vec<(&str, M)> = vec![
            ("host class", Box::new(move |dom: &mut Dom| dom.set_attribute(host, "class", "h0").unwrap())),
            ("textarea rows", Box::new(move |dom: &mut Dom| dom.set_attribute(ta, "rows", "4").unwrap())),
            ("select class", Box::new(move |dom: &mut Dom| dom.set_attribute(sel, "class", "s0").unwrap())),
            ("option selected", Box::new(move |dom: &mut Dom| dom.set_option_selected(o2, true).unwrap())),
            ("input class", Box::new(move |dom: &mut Dom| dom.set_attribute(input, "class", "i0").unwrap())),
            ("img alt", Box::new(move |dom: &mut Dom| dom.set_attribute(img, "alt", "a0").unwrap())),
            ("br class", Box::new(move |dom: &mut Dom| dom.set_attribute(br, "class", "b0").unwrap())),
            ("p class", Box::new(move |dom: &mut Dom| dom.set_attribute(p, "class", "p0").unwrap())),
        ];
        let mut full = Vec::new();
        for (name, m) in &probes {
            let since = dom.borrow().version();
            m(&mut dom.borrow_mut());
            let fresh = dom.borrow().render_tree(d, &OPTS).unwrap();
            match dom.borrow().render_delta(d, since, &OPTS) {
                Some(delta) => {
                    node_ops_match_fresh(&delta.ops, &fresh);
                }
                None => full.push(*name),
            }
        }
        assert!(full.is_empty(), "full renders: {full:?}");
    }
    let since = dom.borrow().version();
    {
        let mut dom = dom.borrow_mut();
        dom.set_attribute(host, "class", "h").unwrap();
        dom.set_attribute(ta, "rows", "5").unwrap();
        dom.set_attribute(sel, "class", "s").unwrap();
        dom.set_option_selected(o2, true).unwrap();
        dom.set_attribute(input, "class", "i").unwrap();
        dom.set_attribute(img, "alt", "a").unwrap();
        dom.set_attribute(br, "class", "b").unwrap();
        dom.set_attribute(p, "class", "p").unwrap();
    }
    let fresh = dom.borrow().render_tree(d, &OPTS).unwrap();
    let delta = dom.borrow().render_delta(d, since, &OPTS).expect("incremental");
    let (nodes, children) = node_ops_match_fresh(&delta.ops, &fresh);
    assert!(children.is_empty(), "{children:?}");
    // Each changed element alone; the option's state change is its own op
    // (and its `<select>`'s, whose validity may follow).
    for key in [render_key(host), render_key(ta), render_key(sel), render_key(input), render_key(img), render_key(br), render_key(p), render_key(o2)] {
        assert!(nodes.contains(&key), "{key:#x} missing from {nodes:?}");
    }
    // Nothing synthetic (the textarea's value text) and nothing from the
    // shadow tree is re-sent: those did not change.
    assert!(nodes.iter().all(|k| k & SYNTHETIC_TEXT == 0), "{nodes:?}");
    assert!(!nodes.contains(&render_key(dom.borrow().first_child(p).unwrap().unwrap())));
    // The fresh tree still has the textarea's typed value and the input's.
    assert!(fresh.nodes.iter().any(|n| n.key == (render_key(ta) | SYNTHETIC_TEXT) && matches!(&n.kind, RenderNodeKind::Text(t) if t == "typed")));
    let input_node = fresh.nodes.iter().find(|n| n.key == render_key(input)).unwrap();
    assert!(matches!(&input_node.kind, RenderNodeKind::Element { attrs, .. } if attrs.iter().any(|(n, v)| &*n.local == "value" && v == "typed in")), "{input_node:?}");
}

#[test]
fn text_content_set_to_itself_leaves_no_ops_and_set_differently_one() {
    let (dom, d) = doc("<body><div id=a><p id=p>same</p></div></body>");
    let (a, p) = (find(&dom.borrow(), d, "#a"), find(&dom.borrow(), d, "#p"));
    let text = dom.borrow().first_child(p).unwrap().unwrap();
    let since = dom.borrow().version();
    dom.borrow_mut().set_text_content(p, "same").unwrap();
    let delta = dom.borrow().render_delta(d, since, &OPTS).expect("incremental");
    assert!(delta.ops.is_empty(), "{:?}", delta.ops);
    // Changed along with its container's attribute: the text node is
    // replaced (a Children op for `p`), the container alone.
    {
        let mut dom = dom.borrow_mut();
        dom.set_attribute(a, "class", "x").unwrap();
        dom.set_text_content(p, "other").unwrap();
    }
    let fresh = dom.borrow().render_tree(d, &OPTS).unwrap();
    let delta = dom.borrow().render_delta(d, since, &OPTS).expect("incremental");
    let (nodes, children) = node_ops_match_fresh(&delta.ops, &fresh);
    assert_eq!(nodes, [render_key(a)]);
    assert_eq!(children, [Some(render_key(p))]);
    assert_ne!(dom.borrow().first_child(p).unwrap(), Some(text));
}

#[test]
fn a_document_with_a_shadow_root_falls_back_to_a_full_render() {
    let (dom, d) = doc("<body><div id=host><span slot=s>light</span></div><p id=p>t</p></body>");
    let (host, p) = (find(&dom.borrow(), d, "#host"), find(&dom.borrow(), d, "#p"));
    {
        let mut dom = dom.borrow_mut();
        let sr = dom.attach_shadow(host, ShadowRootMode::Open).unwrap();
        let frag = parse_fragment(&mut dom, host, "<b>shadow</b><slot name=s></slot>").unwrap();
        dom.append_child(sr, frag).unwrap();
    }
    let since = dom.borrow().version();
    dom.borrow_mut().set_attribute(p, "class", "x").unwrap();
    assert!(dom.borrow().render_delta(d, since, &OPTS).is_none());
    let fresh = dom.borrow().render_tree(d, &OPTS).unwrap();
    assert!(fresh.nodes.iter().any(|n| matches!(&n.kind, RenderNodeKind::Text(t) if t == "shadow")));
}

/// WPT shadow-dom/crashtests/imperative-slot-api-crash.html: two slots of one name (here the
/// default) in one shadow tree. Only the first is assigned the host's children ("find a slot");
/// rendered under both, they were two nodes of one key and the page could not be painted. A
/// `slotAssignment: "manual"` root assigns by `assign()` alone, which holds no state yet: its
/// slots render their fallback content.
#[test]
fn a_host_child_renders_under_one_slot() {
    fn texts(tree: &RenderTree) -> Vec<&str> {
        tree.nodes.iter().filter_map(|n| if let RenderNodeKind::Text(t) = &n.kind { Some(t.as_str()) } else { None }).collect()
    }
    fn keys_are_unique(tree: &RenderTree) -> bool {
        let mut keys: Vec<u64> = tree.nodes.iter().map(|n| n.key).collect();
        keys.sort_unstable();
        keys.windows(2).all(|w| w[0] != w[1])
    }
    for (flags, expected) in [
        (0, vec!["[", "light", "|", "second", "|", "named", "]"]),
        (shadow_flags::MANUAL_SLOTS, vec!["[", "first", "|", "second", "|", "fallback", "]"]),
    ] {
        let (dom, d) = doc("<body><div id=host><span>light</span><i slot=n>named</i></div></body>");
        let host = find(&dom.borrow(), d, "#host");
        {
            let mut dom = dom.borrow_mut();
            let sr = dom.attach_shadow_with(host, ShadowRootMode::Open, flags).unwrap();
            let shadow = "[<slot>first</slot>|<slot>second</slot>|<slot name=n>fallback</slot><slot name=n></slot>]";
            let frag = parse_fragment(&mut dom, host, shadow).unwrap();
            dom.append_child(sr, frag).unwrap();
        }
        let tree = dom.borrow().render_tree(d, &OPTS).unwrap();
        assert_eq!(texts(&tree), expected, "flags {flags}");
        assert!(keys_are_unique(&tree), "flags {flags}");
    }
}

/// A shadow host's light `<style>` children that no slot takes are rendered (in a synthetic slot),
/// those a slot takes once, under it; the slots are looked up once per host, not once per style,
/// so many styles beside a large shadow tree render in bounded time. Review cycle 4, finding 2.
#[test]
fn a_hosts_unassigned_styles_render_once_each_in_bounded_time() {
    fn style_texts(tree: &RenderTree) -> Vec<&str> {
        let styles: Vec<u32> = (0..tree.nodes.len() as u32)
            .filter(|&i| matches!(&tree.nodes[i as usize].kind, RenderNodeKind::Element { name, .. } if &*name.local == "style"))
            .collect();
        tree.nodes
            .iter()
            .filter(|n| styles.contains(&n.parent))
            .filter_map(|n| if let RenderNodeKind::Text(t) = &n.kind { Some(t.as_str()) } else { None })
            .collect()
    }
    let (dom, d) = doc("<body><div id=host><style>.a{}</style><style slot=s>.b{}</style><style slot=x>.c{}</style></div></body>");
    let host = find(&dom.borrow(), d, "#host");
    {
        let mut dom = dom.borrow_mut();
        let sr = dom.attach_shadow(host, ShadowRootMode::Open).unwrap();
        let frag = parse_fragment(&mut dom, host, "<b>shadow</b><slot name=s></slot>").unwrap();
        dom.append_child(sr, frag).unwrap();
    }
    let tree = dom.borrow().render_tree(d, &OPTS).unwrap();
    let mut texts = style_texts(&tree);
    texts.sort_unstable();
    assert_eq!(texts, [".a{}", ".b{}", ".c{}"]);

    let styles = "<style>.x{}</style>".repeat(5000);
    let (dom, d) = doc(&format!("<body><div id=host>{styles}</div></body>"));
    let host = find(&dom.borrow(), d, "#host");
    {
        let mut dom = dom.borrow_mut();
        let sr = dom.attach_shadow(host, ShadowRootMode::Open).unwrap();
        let frag = parse_fragment(&mut dom, host, &"<i>s</i>".repeat(20000)).unwrap();
        dom.append_child(sr, frag).unwrap();
    }
    let start = std::time::Instant::now();
    let tree = dom.borrow().render_tree(d, &OPTS).unwrap();
    assert!(start.elapsed() < std::time::Duration::from_secs(2), "took {:?}", start.elapsed());
    assert_eq!(style_texts(&tree).len(), 5000);
}
