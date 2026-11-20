//! Hit testing by descending the engine's tree (`GeometrySource::hit_candidates`,
//! pyro-render's `hit` module) must answer exactly what the scan of every box
//! of the document answers (`Layout::element_from_point_scan`), on pages
//! that exercise every way a box can lie outside its ancestors' bounds:
//! tables (rowspans, collapsed borders, overflowing cells), nested scroll
//! containers with scroll offsets, transforms, stacking contexts, floats,
//! inline elements over several lines with tight line heights, pointer
//! events, visibility, inert, iframes, shadow trees, SVG, sticky and fixed
//! boxes under a window scroll.
use std::cell::RefCell;
use std::rc::Rc;

use pyro_dom::layout::Layout;
use pyro_dom::media::Viewport;
use pyro_dom::node::ShadowRootMode;
use pyro_dom::parse::{parse_document, set_inner_html, DomRef};
use pyro_dom::{Dom, NodeId};

struct NoFetch;
impl pyro_render::Fetcher for NoFetch {
    fn fetch(&self, url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(Err(format!("{url}: offline")));
    }
}

pub(crate) struct Page {
    pub(crate) dom: Dom,
    pub(crate) doc: NodeId,
    pub(crate) layout: Layout,
}

#[test]
fn hit_each_child_fragment_through_its_spanner_continuation() {
    for (wrapped, layer, under_overlap) in [(true, "", false), (false, "", true), (true, "position:relative", true)] {
        let (open, close) = if wrapped { ("<div>", "</div>") } else { ("", "") };
        let page = page(&format!("<!doctype html><style>body{{margin:0}}section{{width:100px;columns:2;column-gap:0}}header{{height:60px}}aside{{column-span:all;height:40px}}article{{width:120px;height:100px;margin-top:-70px;{layer}}}</style><section>{open}<header id=h></header><aside id=s></aside><article id=a></article>{close}</section>"));
        for (x, y, target) in [
            (10., 10., if under_overlap { "a" } else { "h" }),
            (60., 10., if under_overlap { "a" } else { "h" }),
            (10., 40., if under_overlap { "a" } else { "s" }),
            (10., 77., "a"),
            (155., 77., "a"),
        ] {
            let expected = Some(page.id(target));
            assert_eq!(page.layout.element_from_point(&page.dom, x, y), expected, "wrapped={wrapped} {layer} ({x},{y})");
            assert_eq!(page.layout.element_from_point_scan(&page.dom, x, y), expected);
            assert_eq!(page.layout.element_from_point_descent(&page.dom, x, y), expected);
        }
    }
}

/// Fills shadow root `root` with `html` (parsed in a scratch element).
pub(crate) fn fill_shadow(p: &mut Page, root: NodeId, html: &str) {
    let scratch = p.dom.create_element(p.doc, "div").unwrap();
    set_inner_html(&mut p.dom, scratch, html).unwrap();
    while let Some(child) = p.dom.first_child(scratch).unwrap() {
        p.dom.append_child(root, child).unwrap();
    }
}

pub(crate) fn page(html: &str) -> Page {
    pyro_layout::install(std::sync::Arc::new(NoFetch));
    let dom_ref: DomRef = Rc::new(RefCell::new(Dom::new()));
    let doc = parse_document(&dom_ref, "http://test/", html);
    let mut dom = Rc::try_unwrap(dom_ref).ok().expect("sole owner").into_inner();
    dom.set_document_active(doc, true).unwrap();
    let mut layout = Layout::with_viewport(doc, Viewport::new(800, 600));
    layout.style_mut().discover_style_elements(&dom);
    Page { dom, doc, layout }
}

impl Page {
    pub(crate) fn id(&self, id: &str) -> NodeId {
        self.dom.get_element_by_id(self.doc, id).unwrap().unwrap_or_else(|| panic!("#{id}"))
    }

    pub(crate) fn describe(&self, node: Option<NodeId>) -> String {
        let Some(n) = node else { return "none".into() };
        let e = self.dom.element(n).ok();
        let tag = e.map(|e| e.name.local.to_string()).unwrap_or_else(|| "?".into());
        let id = self.dom.get_attribute(n, "id").ok().flatten().unwrap_or_default();
        let class = self.dom.get_attribute(n, "class").ok().flatten().unwrap_or_default();
        format!("<{tag} id={id:?} class={class:?} #{}>", n.index())
    }

    /// The descent's answer against the scan's at every point of a grid
    /// over the viewport and at pseudo-random points; `label` names the
    /// page in failures. Returns how many distinct elements were hit (the
    /// page must exercise more than its root).
    pub(crate) fn compare(&self, label: &str) -> usize {
        let mut distinct = std::collections::HashSet::new();
        let mut points = Vec::new();
        let mut y = 1.0;
        while y < 600.0 {
            let mut x = 1.0;
            while x < 800.0 {
                points.push((x, y));
                x += 17.0;
            }
            y += 13.0;
        }
        // Deterministic pseudo-random points (and some on half pixels).
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        for _ in 0..300 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let x = ((seed >> 33) % 1600) as f64 / 2.0;
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let y = ((seed >> 33) % 1200) as f64 / 2.0;
            points.push((x, y));
        }
        let mut mismatches = Vec::new();
        for &(x, y) in &points {
            // As pages ask (a descent, or the index once this layout has
            // one), then each path by itself.
            let asked = self.layout.element_from_point(&self.dom, x, y);
            let scan = self.layout.element_from_point_scan(&self.dom, x, y);
            let descent = self.layout.element_from_point_descent(&self.dom, x, y);
            let indexed = self.layout.element_from_point_indexed(&self.dom, x, y);
            if scan != descent || scan != indexed || scan != asked {
                mismatches.push(format!(
                    "{label} at ({x}, {y}): scan {} descent {} index {} asked {}",
                    self.describe(scan),
                    self.describe(descent),
                    self.describe(indexed),
                    self.describe(asked)
                ));
            }
            distinct.insert(scan);
        }
        // The vertical hit test (WebDriver scroll actions) too.
        for y in (1..600).step_by(9) {
            let y = y as f64;
            let scan = self.layout.element_from_vertical_point_scan(&self.dom, y);
            let descent = self.layout.element_from_vertical_point(&self.dom, y);
            if scan != descent {
                mismatches.push(format!(
                    "{label} vertical at {y}: scan {} descent {}",
                    self.describe(scan),
                    self.describe(descent)
                ));
            }
        }
        assert!(
            mismatches.is_empty(),
            "{} mismatches, first:\n{}",
            mismatches.len(),
            mismatches[..mismatches.len().min(12)].join("\n")
        );
        distinct.len()
    }
}

fn table_html(rows: usize, collapse: bool) -> String {
    let mut body = String::new();
    for r in 0..rows {
        body.push_str("<tr>");
        for c in 0..6 {
            if r % 7 == 3 && c == 1 {
                body.push_str(&format!("<td rowspan=2 class=span>r{r}c{c} <span>spanning</span></td>"));
            } else if r % 7 == 4 && c == 1 {
                // Covered by the rowspan above.
            } else if r % 5 == 2 && c == 4 {
                body.push_str(&format!("<td colspan=2>r{r}c{c} <b>wide</b></td>"));
            } else if r % 5 == 2 && c == 5 {
            } else if r % 11 == 6 && c == 2 {
                body.push_str(&format!(
                    "<td>r{r}c{c}<div style=\"height:40px;width:150px;background:#eee\" class=tall>overflows</div></td>"
                ));
            } else if r % 9 == 1 && c == 3 {
                body.push_str(&format!(
                    "<td style=\"position:relative\">r{r}c{c}<div class=pop style=\"position:absolute;left:30px;top:-10px;width:60px;height:30px;background:#fcc\">pop</div></td>"
                ));
            } else {
                body.push_str(&format!("<td>r{r}c{c} <a href=#>link</a></td>"));
            }
        }
        body.push_str("</tr>");
    }
    format!(
        "<!doctype html><html><head><style>
          body {{ margin: 0; font: 12px/14px sans-serif }}
          table {{ border-collapse: {}; border-spacing: 2px; width: 700px }}
          td, th {{ border: 1px solid #888; padding: 1px 3px; white-space: nowrap; overflow: hidden; height: 16px }}
          thead th {{ position: sticky; top: 0; background: #ddd }}
          tr:nth-child(3n) td {{ border-top: 3px solid #333 }}
        </style></head><body>
        <table id=t><thead><tr><th>A<th>B<th>C<th>D<th>E<th>F</tr></thead><tbody id=tb>{body}</tbody>
        <tfoot><tr><td colspan=6>footer <span>text</span></td></tr></tfoot></table>
        <p>After the table <span id=after>inline text</span>.</p></body></html>",
        if collapse { "collapse" } else { "separate" }
    )
}

#[test]
fn tables_match_the_scan() {
    for (collapse, label) in [(false, "separate"), (true, "collapsed")] {
        let p = page(&table_html(40, collapse));
        assert!(p.compare(&format!("table {label}")) > 50);
        // Scrolled window: sticky headers move, fixed boxes do not.
        let mut p = p;
        p.layout.window_scroll_to(&p.dom, None, Some(120));
        assert!(p.compare(&format!("table {label} scrolled")) > 50);
    }
}

#[test]
fn nested_scroll_containers_match_the_scan() {
    let items: String = (0..40).map(|i| format!("<div class=item>item {i} <span>s{i}</span></div>")).collect();
    let inner_items: String = (0..30).map(|i| format!("<p class=inner>inner {i}</p>")).collect();
    let mut p = page(&format!(
        "<!doctype html><html><head><style>
          body {{ margin: 10px; font: 14px/18px sans-serif }}
          #outer {{ width: 400px; height: 300px; overflow: auto; border: 2px solid #000 }}
          .item {{ height: 24px }}
          #inner {{ width: 200px; height: 120px; overflow: auto; margin: 8px; background: #eef }}
          .inner {{ margin: 0; height: 20px }}
          #escape {{ position: absolute; left: 500px; top: 40px; width: 80px; height: 80px; background: #fdd }}
          #clipped {{ position: relative; left: 350px; width: 100px; height: 20px; background: #dfd }}
          #side {{ position: absolute; left: 450px; top: 10px; width: 300px; height: 500px; overflow: hidden }}
          #side div {{ height: 30px }}
        </style></head><body>
        <div id=outer>{items}<div id=inner>{inner_items}</div><div id=escape>escapes</div><div id=clipped>clipped</div>{items}</div>
        <div id=side>{items}</div>
        </body></html>"
    ));
    assert!(p.compare("scrollers at rest") > 10);
    let (outer, inner, side) = (p.id("outer"), p.id("inner"), p.id("side"));
    p.layout.set_scroll_top(&p.dom, outer, 300);
    p.layout.set_scroll_top(&p.dom, inner, 130);
    p.layout.set_scroll_top(&p.dom, side, 200);
    assert!(p.compare("scrollers scrolled") > 10);
    p.layout.set_scroll_top(&p.dom, outer, 1200);
    p.layout.window_scroll_to(&p.dom, None, Some(5));
    assert!(p.compare("scrollers scrolled far") > 5);
}

#[test]
fn transforms_stacking_and_floats_match_the_scan() {
    let p = page(
        "<!doctype html><html><head><style>
          body { margin: 0; font: 13px/15px sans-serif }
          .box { width: 120px; height: 80px; position: absolute; background: #ccc }
          #rot { left: 40px; top: 40px; transform: rotate(30deg) }
          #rot div { width: 40px; height: 40px; background: #a00; transform: translate(60px, 50px) scale(1.5) }
          #scaled { left: 250px; top: 30px; transform: scale(2) translate(10px, 5px); z-index: 2 }
          #skew { left: 500px; top: 60px; transform: skewX(-25deg); opacity: 0.6 }
          #neg { left: 260px; top: 60px; z-index: -1; width: 300px; height: 150px; background: #0a0 }
          #ctx { left: 20px; top: 250px; z-index: 3; width: 300px; height: 150px; background: #aaf }
          #ctx .deep { position: absolute; left: 100px; top: 20px; z-index: 100; width: 250px; height: 60px; background: #ffa }
          #over { left: 200px; top: 240px; z-index: 4; width: 80px; height: 200px; background: #faf }
          #flex { position: absolute; left: 400px; top: 250px; display: flex; width: 350px; height: 120px }
          #flex div { flex: 1; margin: -10px; background: #eee }
          #flex .z { z-index: 1; background: #ddd }
          #floats { position: absolute; left: 0; top: 420px; width: 780px; height: 170px; line-height: 0.7 }
          .fl { float: left; width: 90px; height: 60px; margin: 4px; background: #cfc }
          .fr { float: right; width: 150px; height: 40px; background: #fcf }
          #floats span { font-size: 22px }
          #floats .tight { line-height: 0.4; font-size: 30px }
          #rel { position: relative; top: -30px; left: 300px; display: inline-block; width: 50px; height: 50px; background: #cc0 }
          .iso { isolation: isolate }
        </style></head><body>
        <div id=rot class=box><div></div></div>
        <div id=scaled class=box><p>scaled <b>text</b> inside</p></div>
        <div id=skew class=box>skewed</div>
        <div id=neg class=box>negative</div>
        <div id=ctx class=box><div class=deep>deep <i>z</i></div><p>ctx text</p></div>
        <div id=over class=box></div>
        <div id=flex><div>a</div><div class=z>b</div><div>c</div><div class=iso>d <span>e</span></div></div>
        <div id=floats><div class=fl></div><div class=fr></div><div class=fl></div>
          Lorem ipsum <span>dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor incididunt ut labore et dolore magna aliqua. Ut enim ad minim veniam, quis nostrud</span> exercitation
          <span class=tight>ullamco laboris nisi ut aliquip ex ea commodo consequat. Duis aute irure dolor in reprehenderit</span>
          <span id=rel></span> in voluptate velit esse cillum dolore eu fugiat nulla pariatur. <a href=#>Excepteur sint</a> occaecat cupidatat non proident, sunt in culpa qui officia deserunt mollit anim id est laborum.
        </div>
        </body></html>",
    );
    assert!(p.compare("transforms") > 15);
}

#[test]
fn pointer_events_visibility_inert_and_replaced_boxes_match_the_scan() {
    let p = page(
        "<!doctype html><html><head><style>
          body { margin: 0; font: 14px/16px sans-serif }
          .cell { position: absolute; width: 150px; height: 100px; background: #ddd }
          #pe { left: 10px; top: 10px; pointer-events: none }
          #pe .auto { pointer-events: auto; margin: 20px; width: 60px; height: 40px; background: #f88 }
          #vh { left: 200px; top: 10px; visibility: hidden }
          #vh .vis { visibility: visible; margin: 30px; width: 60px; height: 30px; background: #8f8 }
          #in { left: 400px; top: 10px }
          #cont { left: 600px; top: 10px; display: contents }
          #cont div { position: absolute; left: 600px; top: 10px; width: 100px; height: 100px; background: #88f; opacity: .5 }
          #none { left: 10px; top: 150px; display: none }
          #replaced { position: absolute; left: 10px; top: 150px; width: 760px; height: 120px }
          iframe, img, svg, canvas, video { width: 100px; height: 80px; margin: 10px; vertical-align: top }
          #svg rect { fill: #0a0 }
          #forms { position: absolute; left: 10px; top: 300px; width: 760px }
          #clip { position: absolute; left: 10px; top: 420px; width: 200px; height: 150px; background: #fda; clip-path: inset(20px 40px) }
          #clip div { position: absolute; left: 150px; top: 100px; width: 80px; height: 80px; background: #adf }
          dialog { left: 400px; top: 420px; margin: 0; width: 200px; height: 100px }
        </style></head><body>
        <div id=pe class=cell>none <div class=auto>auto</div></div>
        <div id=vh class=cell>hidden <div class=vis>visible</div></div>
        <div id=in class=cell inert>inert <button>button</button></div>
        <div id=cont class=cell><div>contents child</div></div>
        <div id=none class=cell>gone</div>
        <div id=replaced><iframe id=frame srcdoc=\"<p>frame</p>\"></iframe><img id=img alt=img><svg id=svg viewBox=\"0 0 100 80\"><rect x=10 y=10 width=50 height=50></rect><circle cx=80 cy=60 r=15></circle></svg><canvas id=canvas></canvas><video id=video></video></div>
        <div id=forms><input id=text value=hello><textarea id=ta>area</textarea><select id=sel><option>one<option>two</select><button id=btn>Press <span>me</span></button><input type=checkbox id=cb><label for=cb>check</label><progress value=0.3></progress></div>
        <div id=clip>clipped by path <div>out</div></div>
        <dialog id=dlg open>dialog <span>text</span></dialog>
        </body></html>",
    );
    assert!(p.compare("pointer events and replaced boxes") > 12);
}

#[test]
fn shadow_trees_match_the_scan() {
    let mut p = page(
        "<!doctype html><html><head><style>
          body { margin: 0; font: 14px/16px sans-serif }
          #host { position: absolute; left: 20px; top: 20px; width: 400px; height: 300px; background: #eee }
          #host2 { position: absolute; left: 450px; top: 20px; width: 300px; height: 500px; overflow: auto; background: #efe }
          .light { height: 30px; background: #ccf }
        </style></head><body>
        <div id=host><div class=light id=l1>light one <span>s</span></div><div class=light slot=b id=l2>light two</div><span id=l3>unslotted</span></div>
        <div id=host2><div class=light>in scroller</div></div>
        <p id=below style=\"position:absolute;left:20px;top:350px\">below <span>the host</span></p>
        </body></html>",
    );
    let host = p.id("host");
    let root = p.dom.attach_shadow(host, ShadowRootMode::Open).unwrap();
    fill_shadow(
        &mut p,
        root,
        "<style>.wrap { padding: 10px; border: 2px solid #999 } .over { position: absolute; left: -10px; top: 250px; width: 200px; height: 80px; background: #fcc; z-index: 1 }</style>
         <div class=wrap id=w1>before <slot></slot> after <b>bold</b></div>
         <div class=wrap id=w2><slot name=b>fallback</slot></div>
         <div class=over>shadow overlay <i>text</i></div>",
    );
    let host2 = p.id("host2");
    let root2 = p.dom.attach_shadow(host2, ShadowRootMode::Open).unwrap();
    let rows: String = (0..30).map(|i| format!("<p style='margin:0;height:24px'>shadow row {i}</p>")).collect();
    fill_shadow(&mut p, root2, &format!("<div id=sc style='height:800px'>{rows}<slot></slot></div>"));
    assert!(p.compare("shadow trees") > 8);
    p.layout.set_scroll_top(&p.dom, host2, 300);
    p.layout.window_scroll_to(&p.dom, None, Some(30));
    assert!(p.compare("shadow trees scrolled") > 8);
}

#[test]
fn sticky_and_fixed_boxes_match_the_scan() {
    let items: String =
        (0..60).map(|i| format!("<section><h3>heading {i}</h3><p>para {i} <span>text</span></p></section>")).collect();
    let mut p = page(&format!(
        "<!doctype html><html><head><style>
          body {{ margin: 0; font: 14px/18px sans-serif }}
          h3 {{ position: sticky; top: 0; margin: 0; background: #ddd }}
          #bar {{ position: fixed; left: 0; right: 0; bottom: 0; height: 40px; background: #333; color: #fff }}
          #bar span {{ position: absolute; right: 10px; top: 10px }}
          #scroller {{ position: fixed; right: 10px; top: 10px; width: 200px; height: 300px; overflow: auto; background: #eee }}
          #scroller h4 {{ position: sticky; top: 0; margin: 0; background: #ccc }}
          #scroller p {{ margin: 0; height: 40px }}
          #sidebar {{ position: sticky; top: 100px; float: right; width: 150px; height: 200px; background: #fdd }}
        </style></head><body>
        <div id=sidebar>sticky sidebar</div>
        {items}
        <div id=bar>fixed bar <span>right</span></div>
        <div id=scroller><h4>sticky in scroller</h4>{}</div>
        </body></html>",
        (0..20).map(|i| format!("<p>row {i}</p>")).collect::<String>()
    ));
    assert!(p.compare("sticky at rest") > 10);
    let scroller = p.id("scroller");
    p.layout.set_scroll_top(&p.dom, scroller, 150);
    p.layout.window_scroll_to(&p.dom, None, Some(700));
    assert!(p.compare("sticky scrolled") > 10);
    p.layout.window_scroll_to(&p.dom, None, Some(2500));
    assert!(p.compare("sticky scrolled far") > 10);
}

#[test]
fn a_mutation_between_hit_tests_is_seen() {
    let mut p = page(&table_html(12, false));
    let target = p.id("tb");
    let first_cell = p.dom.query_selector(p.doc, &pyro_dom::select::parse_selectors("#tb td").unwrap()).unwrap().unwrap();
    let r = p.layout.bounding_client_rect(&p.dom, first_cell);
    let (x, y) = (r.x + 2.0, r.y + 2.0);
    let hit = p.layout.element_from_point(&p.dom, x, y);
    assert_eq!(hit, Some(first_cell), "{} at ({x}, {y}) rect {r:?}", p.describe(hit));
    p.dom.set_attribute(target, "style", "margin-top: 200px; display: block").unwrap();
    assert_eq!(p.layout.element_from_point(&p.dom, x, y), p.layout.element_from_point_scan(&p.dom, x, y));
    assert_ne!(p.layout.element_from_point(&p.dom, x, y), Some(first_cell));
    assert!(p.compare("table after mutation") > 5);
}

/// A static flex item overflowing its container downward paints over the next empty block's
/// background (it paints with the inline content: blitz-dom `Node::items_in_phases`), so it takes
/// the hit there too; the block keeps it beside the item. The block took it (siblings were hit in
/// reverse list order, not in their paint phases). Review cycle 2, finding 2 (8fcda743).
#[test]
fn an_overflowing_flex_item_is_hit_over_the_next_blocks_background() {
    let p = page(
        "<!doctype html><body style='margin:0'><div>\
         <div style='display:flex;height:50px;border:1px solid blue'><div id=item style='width:100px;height:120px;background:green;align-self:flex-start'></div></div>\
         <div id=next style='height:100px;background:yellow;border:1px solid red'></div></div>",
    );
    let (item, next) = (p.id("item"), p.id("next"));
    assert_eq!(p.layout.element_from_point(&p.dom, 50.0, 90.0), Some(item), "over the next block's background");
    assert_eq!(p.layout.element_from_point(&p.dom, 150.0, 90.0), Some(next), "beside the item");
    assert!(p.compare("overflowing flex item") > 2);
}

/// A positioned sibling after a flex container paints over the container's overflowing item (the
/// positioned level comes after the inline content the item paints with): it takes the hit there.
/// The item pre-pass returned before it. Review cycle 3, finding 4.
#[test]
fn a_positioned_sibling_over_an_overflowing_flex_item_is_hit() {
    let p = page(
        "<!doctype html><body style='margin:0'><div style='overflow:hidden'>\
         <div style='display:flex;height:50px;border:1px solid blue'><div id=item style='width:100px;height:120px;background:green;align-self:flex-start'></div></div>\
         <div id=next style='position:relative;height:100px;background:yellow;border:1px solid red'></div></div>",
    );
    let (item, next) = (p.id("item"), p.id("next"));
    assert_eq!(p.layout.element_from_point(&p.dom, 50.0, 90.0), Some(next), "the positioned sibling over the item");
    assert_eq!(p.layout.element_from_point(&p.dom, 50.0, 30.0), Some(item), "the item above the sibling");
    assert!(p.compare("positioned sibling over a flex item") > 1);
}
