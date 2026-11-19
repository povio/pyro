//! An out-of-flow box whose containing block is the initial containing block
//! is a relayout boundary: a change inside it relays the box (from the root
//! positioning pass) while the root's in-flow layout comes from its cache.
//! Every change must still give the boxes, scroll extents and pixels of a
//! document built from scratch.
use crate::incremental_construction::{paint, Page};
use pyro_dom::snapshot::render_key;
use pyro_render::*;

/// Applies `change` to `html`, relayouts incrementally and compares with a
/// from-scratch build. Returns the layout algorithm runs the incremental
/// relayout made (all, and the root element's).
#[track_caller]
fn check(what: &str, html: &str, change: &dyn Fn(&Page)) -> (u64, u64) {
    let page = Page::new(html);
    let mut live = Renderer::new();
    paint(&mut live, &page.snapshot());
    change(&page);
    let snap = page.snapshot();
    let before = blitz_performed_layouts();
    let incremental = paint(&mut live, &snap);
    let after = blitz_performed_layouts();
    assert_eq!(incremental.timings.mirror.unwrap().rebuilt, None, "{what}: synced as changes");
    let mut fresh = Renderer::new();
    let scratch = paint(&mut fresh, &snap);
    let (a, b) = (live.mirrored_layout(1), fresh.mirrored_layout(1));
    for (i, (x, y)) in a.iter().zip(&b).enumerate() {
        assert_eq!(x, y, "{what}: box {i} differs\n{}", a.join("\n"));
    }
    assert_eq!(a.len(), b.len(), "{what}: box count");
    // Scroll extents (the root's scrollable overflow includes the boxes the
    // root positioning pass lays out).
    for css in ["html", "body", "#m"] {
        let key = render_key(page.find(css));
        let (x, y) = (live.element_box(1, key), fresh.element_box(1, key));
        assert_eq!(x, y, "{what}: {css} box and scroll extents");
    }
    assert_eq!(incremental.hash, scratch.hash, "{what}: pixels");
    (after.0 - before.0, after.1 - before.1)
}

fn append_paragraph(p: &Page, parent: &str, text: &str) {
    let parent = p.find(parent);
    let mut d = p.dom.borrow_mut();
    let para = d.create_element(p.doc, "p").unwrap();
    let t = d.create_text(p.doc, text).unwrap();
    d.append_child(para, t).unwrap();
    d.append_child(parent, para).unwrap();
}

/// A page of in-flow content and Paprize's measuring container: an
/// absolutely positioned box at the root that clones are appended to.
fn page(container: &str) -> String {
    format!(
        r#"<!doctype html><style>body{{margin:0;font:14px sans-serif}} p{{margin:4px 0}} {container}</style>
<main><section><p>one</p><p>two</p><p>three</p></section><div id=flow><p>four</p></div></main>
<div id=m><p id=first>measured</p></div>"#
    )
}

#[test]
fn a_change_inside_a_root_abspos_box_leaves_the_root_layout_cached() {
    for (what, css) in [
        ("set insets", "#m{position:absolute;top:10px;left:300px;width:200px}"),
        ("auto insets", "#m{position:absolute;width:200px}"),
        ("fit-content width", "#m{position:absolute;top:0;left:0}"),
        ("position:fixed", "#m{position:fixed;bottom:0;right:0}"),
    ] {
        let (_, root) = check(&format!("{what}: a paragraph appended"), &page(css), &|p| {
            append_paragraph(p, "#m", "a far longer paragraph which wraps over more than one line of the box");
        });
        assert_eq!(root, 0, "{what}: the root's in-flow layout is not run again");
        let (_, root) = check(&format!("{what}: text edited"), &page(css), &|p| {
            let t = p.text("#first");
            p.dom.borrow_mut().replace_data(t, 0, 8, "measured again, and wider").unwrap();
        });
        assert_eq!(root, 0, "{what}: text edited");
        let (_, root) = check(&format!("{what}: a child removed"), &page(css), &|p| {
            let (m, first) = (p.find("#m"), p.find("#first"));
            p.dom.borrow_mut().remove_child(m, first).unwrap();
        });
        assert_eq!(root, 0, "{what}: a child removed");
    }
}

#[test]
fn the_boundary_relays_only_the_box() {
    // Ancestors keep their layout: the box and the appended paragraph are the
    // only layout algorithm runs, none of the in-flow page's seven boxes. (The
    // box's auto height is no percentage basis, so its first paragraph is laid
    // out at the inputs it was measured at and comes from its cache.)
    let (runs, root) = check("only the box", &page("#m{position:absolute;top:0;left:300px;width:200px}"), &|p| {
        append_paragraph(p, "#m", "appended");
    });
    assert_eq!(root, 0);
    assert_eq!(runs, 2, "the container and the appended paragraph");
}

#[test]
fn nested_out_of_flow_boxes() {
    // The inner box's containing block is the outer box, which relays it;
    // the outer box is still a boundary for the root.
    let html = r#"<!doctype html><style>body{margin:0;font:14px sans-serif}
#m{position:absolute;top:0;left:200px;width:300px;height:200px} #n{position:absolute;bottom:0;right:0}</style>
<p>in flow</p><div id=m><p>outer</p><div id=n><p id=inner>inner</p></div></div>"#;
    let (_, root) = check("text inside an abspos inside an abspos", html, &|p| {
        let t = p.text("#inner");
        p.dom.borrow_mut().replace_data(t, 0, 5, "much wider inner text").unwrap();
    });
    assert_eq!(root, 0);
    // A fixed box surfaces from the outer box to the root pass: it must be
    // hoisted by the root although the root's own layout is cached.
    let (_, root) = check("a fixed box added inside a root abspos box", html, &|p| {
        let n = p.find("#n");
        let mut d = p.dom.borrow_mut();
        let f = d.create_element(p.doc, "div").unwrap();
        d.set_attribute(f, "style", "position:fixed;top:300px;left:20px;width:50px;height:40px;background:red")
            .unwrap();
        let t = d.create_text(p.doc, "fixed").unwrap();
        d.append_child(f, t).unwrap();
        d.append_child(n, f).unwrap();
    });
    assert_eq!(root, 0);
    let html_with_fixed = r#"<!doctype html><style>body{margin:0;font:14px sans-serif}
#m{position:absolute;top:0;left:200px;width:300px} #f{position:fixed;top:300px;left:20px;background:red}</style>
<p>in flow</p><div id=m><p>outer</p><div id=f><p id=inner>fixed</p></div></div>"#;
    check("a fixed box inside a root abspos box removed", html_with_fixed, &|p| {
        let (m, f) = (p.find("#m"), p.find("#f"));
        p.dom.borrow_mut().remove_child(m, f).unwrap();
    });
    check("text inside a fixed box inside a root abspos box", html_with_fixed, &|p| {
        let t = p.text("#inner");
        p.dom.borrow_mut().replace_data(t, 0, 5, "a wider fixed box").unwrap();
    });
}

#[test]
fn the_box_changing_itself_relays_its_parent() {
    // Its own style can move its static position: not a boundary.
    let css = "#m{position:absolute;width:200px} .w{width:350px} .i{top:40px;left:10px} .s{justify-self:end}";
    for (what, class) in [("own width", "w"), ("own inset", "i"), ("own self-alignment", "s")] {
        check(what, &page(css), &|p| {
            let m = p.find("#m");
            p.dom.borrow_mut().set_attribute(m, "class", class).unwrap();
        });
    }
    check("switching to position:static", &page("#m{position:absolute;top:0} .s{position:static}"), &|p| {
        let m = p.find("#m");
        p.dom.borrow_mut().set_attribute(m, "class", "s").unwrap();
    });
    check("switching to position:absolute", &page("#m{width:200px} .a{position:absolute;top:0;left:300px}"), &|p| {
        let m = p.find("#m");
        p.dom.borrow_mut().set_attribute(m, "class", "a").unwrap();
    });
    check("a root abspos box added", &page(""), &|p| {
        let body = p.find("body");
        let mut d = p.dom.borrow_mut();
        let div = d.create_element(p.doc, "div").unwrap();
        d.set_attribute(div, "style", "position:absolute;top:50px;left:50px").unwrap();
        let t = d.create_text(p.doc, "new").unwrap();
        d.append_child(div, t).unwrap();
        d.append_child(body, div).unwrap();
    });
}

#[test]
fn a_nearer_containing_block_relays() {
    for (what, css) in [
        ("a positioned ancestor", "main{position:relative} #m{position:absolute;top:0;right:0}"),
        ("a transformed ancestor", "main{transform:translateX(5px)} #m{position:fixed;top:0;right:0}"),
        ("a size container", "main{container-type:inline-size} #m{position:absolute;top:0;right:0}"),
    ] {
        let html = format!(
            r#"<!doctype html><style>body{{margin:0;font:14px sans-serif}} {css}</style>
<main><p>one</p><div><div id=m><p id=first>measured</p></div></div><p>two</p></main>"#
        );
        check(&format!("{what}: a paragraph appended"), &html, &|p| {
            append_paragraph(p, "#m", "a far longer paragraph which wraps over more than one line");
        });
        check(&format!("{what}: text edited"), &html, &|p| {
            let t = p.text("#first");
            p.dom.borrow_mut().replace_data(t, 0, 8, "measured again, wider").unwrap();
        });
    }
    // A positioned span is the containing block of an abspos box inside it,
    // laid out by its inline root.
    let html = r#"<!doctype html><style>body{margin:0;font:14px sans-serif} span{position:relative} #m{position:absolute;top:20px;left:0}</style>
<p>text <span>span <b id=m><i id=first>measured</i></b></span> more</p>"#;
    check("a positioned span", html, &|p| {
        let t = p.text("#first");
        p.dom.borrow_mut().replace_data(t, 0, 8, "measured again, wider").unwrap();
    });
}

#[test]
fn an_abspos_box_grows_and_shrinks_the_document() {
    // The root's scrollable overflow follows the relaid box.
    let css = "#m{position:absolute;top:300px;left:0;width:200px} #m p{height:200px}";
    let (_, root) = check("taller than the viewport", &page(css), &|p| {
        append_paragraph(p, "#m", "a tall paragraph");
        append_paragraph(p, "#m", "and another");
    });
    assert_eq!(root, 0);
    let html = r#"<!doctype html><style>body{margin:0;font:14px sans-serif} p{margin:0;height:40px}
#m{position:absolute;top:0;left:0;width:100px} #first{height:900px}</style><p>a</p><div id=m><p>1</p><p id=first>2</p></div>"#;
    let (_, root) = check("shorter than the viewport again", html, &|p| {
        let (m, first) = (p.find("#m"), p.find("#first"));
        p.dom.borrow_mut().remove_child(m, first).unwrap();
    });
    assert_eq!(root, 0);
}

#[test]
fn an_abspos_box_in_flex_grid_and_inline_contexts() {
    for (what, css, wrap) in [
        ("flex", ".w{display:flex;gap:4px} #m{position:absolute}", "div class=w"),
        ("grid", ".w{display:grid;grid-template-columns:1fr 1fr} #m{position:absolute;grid-column:2}", "div class=w"),
        ("inline", "#m{position:absolute}", "p"),
        ("anonymous block", "#m{position:absolute}", "div"),
    ] {
        let tag = wrap.split(' ').next().unwrap();
        let html = format!(
            r#"<!doctype html><style>body{{margin:0;font:14px sans-serif}} {css}</style>
<{wrap}>before <span>x</span><span id=m><span id=first>measured</span></span><div>block</div> after</{tag}>"#
        );
        let (_, root) = check(&format!("{what}: text edited"), &html, &|p| {
            let t = p.text("#first");
            p.dom.borrow_mut().replace_data(t, 0, 8, "measured again, and wider").unwrap();
        });
        assert_eq!(root, 0, "{what}");
        check(&format!("{what}: a paragraph appended"), &html, &|p| {
            append_paragraph(p, "#m", "appended paragraph");
        });
    }
}

#[test]
fn content_dependent_static_positions_and_span_containing_blocks() {
    let cases: Vec<(&str, String)> = vec![
        ("flex centered", r#"<style>body{margin:0;font:14px sans-serif} .w{display:flex;justify-content:center;align-items:center;height:300px} #m{position:absolute}</style><div class=w><p>x</p><div id=m><p id=first>measured</p></div></div>"#.into()),
        ("grid centered", r#"<style>body{margin:0;font:14px sans-serif} .w{display:grid;place-items:center;height:300px} #m{position:absolute}</style><div class=w><p>x</p><div id=m><p id=first>measured</p></div></div>"#.into()),
        ("rtl auto insets", r#"<style>body{margin:0;font:14px sans-serif;direction:rtl} #m{position:absolute}</style><p>x</p><div id=m><p id=first>measured</p></div>"#.into()),
        ("text-align center inline", r#"<style>body{margin:0;font:14px sans-serif} p{text-align:center} #m{position:absolute}</style><p>x <span id=m><span id=first>measured</span></span> y</p>"#.into()),
        ("html positioned", r#"<style>html{position:relative} body{margin:0;font:14px sans-serif} #m{position:absolute;top:0;left:0}</style><p>x</p><div id=m><p id=first>measured</p></div>"#.into()),
        ("html transformed", r#"<style>html{transform:translateX(1px)} body{margin:0;font:14px sans-serif} #m{position:absolute;top:0;left:0}</style><p>x</p><div id=m><p id=first>measured</p></div>"#.into()),
        ("scroll container between", r#"<style>body{margin:0;font:14px sans-serif} .s{overflow:auto;height:100px} #m{position:absolute;top:0;left:0}</style><div class=s><p>x</p><div id=m><p id=first>measured</p></div></div>"#.into()),
        ("table cell", r#"<style>body{margin:0;font:14px sans-serif} #m{position:absolute}</style><table><tr><td>x<div id=m><p id=first>measured</p></div></td></tr></table>"#.into()),
        ("positioned span, inline-block, p", r#"<style>body{margin:0;font:14px sans-serif} .r{position:relative} .ib{display:inline-block} #m{position:absolute;top:20px;left:0}</style><p>text <span class=r>s <span class=ib>ib <b id=m><i id=first>measured</i></b></span></span> more</p>"#.into()),
        ("positioned span, inline-block, anonymous root", r#"<style>body{margin:0;font:14px sans-serif} .r{position:relative} .ib{display:inline-block} #m{position:absolute;top:20px;left:0}</style><div>text <span class=r>s <span class=ib>ib <b id=m><i id=first>measured</i></b></span></span> more<div>block</div></div>"#.into()),
        ("positioned span, block in inline", r#"<style>body{margin:0;font:14px sans-serif} .r{position:relative} #m{position:absolute;top:20px;left:0}</style><div>text <span class=r>s <div>blk <b id=m><i id=first>measured</i></b></div></span> more</div>"#.into()),
        ("positioned span, block in inline, deeper", r#"<style>body{margin:0;font:14px sans-serif} .r{position:relative} #m{position:absolute;top:20px;left:0}</style><div>text <span class=r>s <div><section>blk <b id=m><i id=first>measured</i></b></section></div></span> more</div>"#.into()),
        ("fixed in positioned-only ancestor", r#"<style>body{margin:0;font:14px sans-serif} main{position:relative} #m{position:fixed;top:0;left:0}</style><main><p>x</p><div><div id=m><p id=first>measured</p></div></div></main>"#.into()),
        ("pseudo parent", r#"<style>body{margin:0;font:14px sans-serif} #m{position:absolute;top:0;left:0} #m::after{content:"after"}</style><p>x</p><div id=m><p id=first>measured</p></div>"#.into()),
    ];
    for (what, html) in &cases {
        let html = format!("<!doctype html>{html}");
        check(&format!("{what}: text edited"), &html, &|p| {
            let t = p.text("#first");
            p.dom.borrow_mut().replace_data(t, 0, 8, "measured again, and much wider than before").unwrap();
        });
        check(&format!("{what}: paragraph appended"), &html, &|p| {
            append_paragraph(p, "#m", "a far longer paragraph which wraps over more than one line of the box");
        });
        check(&format!("{what}: fixed added"), &html, &|p| {
            let m = p.find("#m");
            let mut d = p.dom.borrow_mut();
            let f = d.create_element(p.doc, "div").unwrap();
            d.set_attribute(f, "style", "position:fixed;top:300px;left:20px;width:50px;height:40px;background:red")
                .unwrap();
            let t = d.create_text(p.doc, "fixed").unwrap();
            d.append_child(f, t).unwrap();
            d.append_child(m, f).unwrap();
        });
    }
}

#[test]
fn an_inline_root_claiming_for_a_positioned_span_elsewhere_in_it() {
    // An inline root with a positioned span holding an inline box claims
    // every out-of-flow box of its subtree (`SPAN_*_CB` in its layout style),
    // also one outside the span, whose DOM ancestors are none of them
    // positioned. That box is relaid by the inline root's out-of-flow pass,
    // which runs only inside the inline root's own layout.
    let cases: Vec<(&str, String)> = vec![
        ("abspos sibling of the span, p", r#"<style>body{margin:0;font:14px sans-serif} .r{position:relative} .ib{display:inline-block} #m{position:absolute;top:20px;left:0}</style><p>text <span class=r>s <span class=ib>ib</span></span> more <b id=m><i id=first>measured</i></b></p>"#.into()),
        ("abspos sibling of the span, anonymous root", r#"<style>body{margin:0;font:14px sans-serif} .r{position:relative} .ib{display:inline-block} #m{position:absolute;top:20px;left:0}</style><div>text <span class=r>s <span class=ib>ib</span></span> more <b id=m><i id=first>measured</i></b><div>block</div></div>"#.into()),
        ("abspos in another span, anonymous root", r#"<style>body{margin:0;font:14px sans-serif} .r{position:relative} .ib{display:inline-block} #m{position:absolute;top:20px;left:0}</style><div>text <span class=r>s <span class=ib>ib</span></span> more <span>in <b id=m><i id=first>measured</i></b></span><div>block</div></div>"#.into()),
        ("fixed sibling of a filtered span, p", r#"<style>body{margin:0;font:14px sans-serif} .r{filter:blur(0)} .ib{display:inline-block} #m{position:fixed;top:20px;left:0}</style><p>text <span class=r>s <span class=ib>ib</span></span> more <b id=m><i id=first>measured</i></b></p>"#.into()),
    ];
    for (what, html) in &cases {
        let html = format!("<!doctype html>{html}");
        check(&format!("{what}: text edited"), &html, &|p| {
            let t = p.text("#first");
            p.dom.borrow_mut().replace_data(t, 0, 8, "measured again, and much wider than before").unwrap();
        });
        check(&format!("{what}: paragraph appended"), &html, &|p| {
            append_paragraph(p, "#m", "a far longer paragraph which wraps over more than one line of the box");
        });
    }
}

#[test]
fn an_ancestor_becoming_or_ceasing_to_be_a_containing_block() {
    for (what, css, class) in [
        (
            "transform removed",
            "main{transform:translateX(5px)} main.n{transform:none} #m{position:fixed;top:0;right:0}",
            "n",
        ),
        ("transform added", "main.n{transform:translateX(5px)} #m{position:fixed;top:0;right:0}", "n"),
        (
            "position removed",
            "main{position:relative} main.n{position:static} #m{position:absolute;top:0;right:0}",
            "n",
        ),
        ("position added", "main.n{position:relative} #m{position:absolute;top:0;right:0}", "n"),
        ("filter removed", "main{filter:blur(0)} main.n{filter:none} #m{position:fixed;top:0;right:0}", "n"),
        (
            "will-change removed",
            "main{will-change:transform} main.n{will-change:auto} #m{position:fixed;top:0;right:0}",
            "n",
        ),
        ("contain removed", "main{contain:paint} main.n{contain:none} #m{position:absolute;top:0;right:0}", "n"),
    ] {
        let html = format!(
            r#"<!doctype html><style>body{{margin:0;font:14px sans-serif}} main{{margin:30px}} {css}</style>
<main><p>one</p><div><div id=m><p id=first>measured</p></div></div><p>two</p></main>"#
        );
        check(&format!("{what} alone"), &html, &|p| {
            let m = p.find("main");
            p.dom.borrow_mut().set_attribute(m, "class", class).unwrap();
        });
        check(&format!("{what} with text edited"), &html, &|p| {
            let m = p.find("main");
            p.dom.borrow_mut().set_attribute(m, "class", class).unwrap();
            let t = p.text("#first");
            p.dom.borrow_mut().replace_data(t, 0, 8, "measured again, wider").unwrap();
        });
    }
}

#[test]
fn several_boundaries_and_in_flow_changes_together() {
    let html = r#"<!doctype html><style>body{margin:0;font:14px sans-serif} #m{position:absolute;inset:0;margin:auto;width:fit-content;height:fit-content} #k{position:absolute;bottom:0;left:0}</style><p id=flowp>flow</p><div id=m><p id=first>measured</p></div><div id=k><p id=second>other</p></div>"#;
    let (_, root) = check("centered with margin auto", html, &|p| {
        let t = p.text("#first");
        p.dom.borrow_mut().replace_data(t, 0, 8, "measured again, and much wider than before").unwrap();
    });
    assert_eq!(root, 0);
    let (_, root) = check("two boundaries at once", html, &|p| {
        append_paragraph(p, "#m", "one more");
        append_paragraph(p, "#k", "and another one which is longer");
    });
    assert_eq!(root, 0);
    check("boundary and in-flow change together", html, &|p| {
        append_paragraph(p, "#m", "one more");
        let t = p.text("#flowp");
        p.dom
            .borrow_mut()
            .replace_data(
                t,
                0,
                4,
                "flow text that is now considerably longer and wraps onto more lines than before, surely, yes",
            )
            .unwrap();
    });
    let html2 = r#"<!doctype html><style>body{margin:0;font:14px sans-serif} #m{position:absolute;top:10px;left:10px}</style><p>flow</p>"#;
    for step in 0..2 {
        check(&format!("abspos child of html, step {step}"), html2, &|p| {
            let root = p.find("html");
            let mut d = p.dom.borrow_mut();
            let div = d.create_element(p.doc, "div").unwrap();
            d.set_attribute(div, "id", "m").unwrap();
            let t = d.create_text(p.doc, "on html").unwrap();
            d.append_child(div, t).unwrap();
            d.append_child(root, div).unwrap();
            if step == 1 {
                let para = d.create_element(p.doc, "p").unwrap();
                let t = d.create_text(p.doc, "more").unwrap();
                d.append_child(para, t).unwrap();
                d.append_child(div, para).unwrap();
            }
        });
    }
}

#[test]
fn an_abspos_child_of_the_root_element_is_a_boundary() {
    let html = r#"<!doctype html><style>body{margin:0;font:14px sans-serif} #m{position:absolute;top:10px;left:10px}</style><p>flow</p>"#;
    let page = Page::new(html);
    let mut live = Renderer::new();
    paint(&mut live, &page.snapshot());
    let t;
    {
        let root = page.find("html");
        let mut d = page.dom.borrow_mut();
        let div = d.create_element(page.doc, "div").unwrap();
        d.set_attribute(div, "id", "m").unwrap();
        t = d.create_text(page.doc, "on html").unwrap();
        d.append_child(div, t).unwrap();
        d.append_child(root, div).unwrap();
    }
    paint(&mut live, &page.snapshot());
    page.dom.borrow_mut().replace_data(t, 0, 2, "much wider text, on").unwrap();
    let snap = page.snapshot();
    let before = blitz_performed_layouts();
    let inc = paint(&mut live, &snap);
    let after = blitz_performed_layouts();
    let mut fresh = Renderer::new();
    let scratch = paint(&mut fresh, &snap);
    assert_eq!(live.mirrored_layout(1), fresh.mirrored_layout(1));
    assert_eq!(inc.hash, scratch.hash);
    assert_eq!(after.1 - before.1, 0, "the root's in-flow layout is not run again");
}
