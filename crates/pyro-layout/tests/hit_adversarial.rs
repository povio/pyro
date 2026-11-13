//! Adversarial checks of hit testing by tree descent (review of
//! `perf/hit-test`): the descent (`Layout::element_from_point`) against the
//! scan of every box (`Layout::element_from_point_scan`) on the cases the
//! descent's bounds are most likely to miss: vertical writing modes (also in
//! tables and flex), 3D transforms, transformed rows and cells, negative
//! margins, `overflow: clip`, `contain: paint`, out-of-flow boxes in inline
//! containing blocks and transformed ancestors, multi-column, generated
//! content, list markers, overflowing text, slotted content placed outside
//! its host, the top layer, and boxes that move or a viewport that resizes
//! between hit tests. Plus randomly generated pages.
use pyro_dom::media::Viewport;
use pyro_dom::node::ShadowRootMode;
use pyro_dom::NodeId;

use crate::hit_descent::{fill_shadow, page, Page};

/// Words of `caret_from_point` and `text_rect` (which read the split-out
/// words) at a grid: the same before and after anything the descent does.
fn caret_grid(p: &Page) -> Vec<Option<(NodeId, u32, bool)>> {
    let body = p.dom.body(p.doc).ok().flatten().unwrap_or(p.doc);
    let mut out = Vec::new();
    let mut y = 3.0;
    while y < 600.0 {
        let mut x = 3.0;
        while x < 800.0 {
            out.push(p.layout.caret_from_point(&p.dom, body, x, y, &|_| true));
            x += 41.0;
        }
        y += 37.0;
    }
    out
}

#[test]
fn vertical_writing_modes_match_the_scan() {
    let mut p = page(
        "<!doctype html><html><head><style>
          body { margin: 0; font: 14px/16px sans-serif }
          .v { position: absolute; width: 200px; height: 260px; background: #eee }
          #rl { left: 10px; top: 10px; writing-mode: vertical-rl }
          #lr { left: 230px; top: 10px; writing-mode: vertical-lr; line-height: 0.6 }
          #rl span, #lr span { background: #fcc }
          #rl .neg { margin-right: -60px; background: #cfc; width: 30px }
          #tbl { position: absolute; left: 450px; top: 10px; writing-mode: vertical-rl; border-collapse: collapse }
          #tbl td { border: 2px solid #888; padding: 2px; overflow: hidden }
          #tbl td div { width: 30px; height: 120px; background: #ccf }
          #flex { position: absolute; left: 10px; top: 300px; display: flex; writing-mode: vertical-lr; height: 250px; width: 300px }
          #flex div { flex: 1; margin: -12px; background: #ffc }
          #flex .z { z-index: 2; position: relative; background: #fcf }
          #rtl { position: absolute; left: 400px; top: 320px; width: 200px; direction: rtl; background: #eef }
          #rtl div { width: 300px; height: 40px; background: #cfe }
          #rtlv { position: absolute; left: 620px; top: 300px; width: 150px; height: 280px; writing-mode: vertical-rl; direction: rtl; overflow: auto }
          #rtlv p { margin: 0; inline-size: 400px; block-size: 50px; background: #dfd }
        </style></head><body>
        <div id=rl class=v>vertical rl text <span>with span wrapping over several lines of text</span> <div class=neg>neg</div> more</div>
        <div id=lr class=v>vertical lr <span>tight lines going on and on</span> and <b>bold</b> words</div>
        <table id=tbl><tr><td>a<div></div><td rowspan=2>span<div></div><td>c</tr><tr><td>d<td><span>e</span></tr></table>
        <div id=flex><div>a</div><div class=z>b</div><div>c</div></div>
        <div id=rtl>rtl <div>wide child</div><span>inline</span></div>
        <div id=rtlv><p>one</p><p>two</p><p>three <span>s</span></p><p>four</p><p>five</p></div>
        </body></html>",
    );
    assert!(p.compare("vertical writing modes") > 15);
    let rtlv = p.id("rtlv");
    p.layout.set_scroll_left(&p.dom, rtlv, -100);
    p.layout.set_scroll_top(&p.dom, rtlv, 80);
    p.layout.window_scroll_to(&p.dom, None, Some(20));
    assert!(p.compare("vertical writing modes scrolled") > 15);
}

#[test]
fn three_d_transforms_and_transformed_table_parts_match_the_scan() {
    let p = page(
        "<!doctype html><html><head><style>
          body { margin: 0; font: 13px/15px sans-serif }
          #persp { position: absolute; left: 20px; top: 20px; width: 300px; height: 200px; perspective: 300px; background: #eee }
          #persp .card { width: 200px; height: 120px; margin: 30px; background: #fcc; transform: rotateY(50deg) }
          #persp .card div { width: 260px; height: 30px; background: #cfc; transform: translateZ(60px) rotateX(20deg) }
          #p3d { position: absolute; left: 380px; top: 20px; width: 200px; height: 200px; transform-style: preserve-3d; transform: perspective(200px) rotateX(35deg) }
          #p3d div { width: 260px; height: 60px; background: #ccf; transform: translateZ(40px) }
          #t { position: absolute; left: 20px; top: 260px; border-collapse: collapse; width: 500px }
          #t td { border: 1px solid #888; height: 30px }
          #t tr.mv { transform: translateX(80px) }
          #t td.rot { transform: rotate(20deg); background: #ffc }
          #t td.far { transform: translate(200px, 60px) }
          #t tbody.sh { transform: translateY(40px) scale(0.8) }
          #t tr.rel { position: relative; left: 100px; top: 5px }
          #t td.rel { position: relative; top: -40px; left: 300px; background: #fdc }
        </style></head><body>
        <div id=persp><div class=card>card <div>front</div></div></div>
        <div id=p3d><div>flat 3d</div></div>
        <table id=t>
          <tbody><tr class=mv><td>a<td class=rot>rot<td>c</tr><tr><td class=far>far<td>e<td>f</tr></tbody>
          <tbody class=sh><tr><td>g<td>h<td rowspan=2>i</tr><tr class=rel><td>j<td class=rel>k</tr></tbody>
          <tbody><tr><td>l<td class=rel>m<td>n</tr></tbody>
        </table>
        </body></html>",
    );
    assert!(p.compare("3d transforms and table parts") > 15);
}

#[test]
fn negative_margins_clip_and_containment_match_the_scan() {
    let p = page(
        "<!doctype html><html><head><style>
          body { margin: 0; font: 13px/15px sans-serif }
          .b { position: absolute; width: 160px; height: 120px; background: #eee }
          .kid { width: 120px; height: 60px; background: #fcc }
          #neg { left: 200px; top: 100px }
          #neg .kid { margin-left: -150px; margin-top: -80px }
          #neg .kid2 { margin-left: -190px; width: 60px; height: 30px; background: #cfc; position: relative; top: -60px }
          #clip { left: 450px; top: 20px; overflow: clip }
          #clip .kid { margin-left: 100px; width: 200px; height: 200px }
          #clipx { left: 450px; top: 200px; overflow-x: clip }
          #clipx .kid { margin-left: 100px; width: 200px; height: 200px }
          #clipm { left: 650px; top: 20px; width: 80px; overflow: clip; overflow-clip-margin: 30px }
          #clipm .kid { margin-left: 40px; width: 120px; height: 160px }
          #paint { left: 20px; top: 300px; contain: paint }
          #paint .kid { margin-left: 120px; width: 200px; height: 200px; background: #ccf }
          #paint .abs { position: absolute; left: 140px; top: 90px; width: 100px; height: 80px; background: #fcf }
          #paint .fix { position: fixed; left: 700px; top: 500px; width: 60px; height: 60px; background: #ffc }
          #layout { left: 350px; top: 380px; contain: layout }
          #layout .abs { position: absolute; left: 140px; top: 90px; width: 100px; height: 80px; background: #cff }
          #hid { left: 600px; top: 380px; overflow: hidden; border-radius: 40px }
          #hid .kid { width: 300px; height: 300px }
          #deco { position: absolute; left: 20px; top: 20px; width: 100px; height: 40px; outline: 30px solid red; box-shadow: 0 0 0 50px blue; background: #888 }
        </style></head><body>
        <div id=neg class=b><div class=kid>pulled</div><div class=kid2>more</div>text <span>after</span></div>
        <div id=clip class=b><div class=kid>clip</div></div>
        <div id=clipx class=b><div class=kid>clip x</div></div>
        <div id=clipm class=b><div class=kid>clip margin</div></div>
        <div id=paint class=b><div class=kid>paint</div><div class=abs>abs</div><div class=fix>fix</div></div>
        <div id=layout class=b><div class=abs>abs</div></div>
        <div id=hid class=b><div class=kid>hidden</div></div>
        <div id=deco>decorated</div>
        </body></html>",
    );
    assert!(p.compare("negative margins, clips, containment") > 15);
}

#[test]
fn out_of_flow_boxes_in_unusual_containing_blocks_match_the_scan() {
    let mut p = page(
        "<!doctype html><html><head><style>
          body { margin: 0; font: 14px/16px sans-serif }
          #para { position: absolute; left: 10px; top: 10px; width: 300px }
          #para .cb { position: relative; background: #eef }
          #para .cb .abs { position: absolute; left: 20px; top: 40px; width: 120px; height: 50px; background: #fcc }
          #para .cb .abs2 { position: absolute; right: -150px; bottom: -120px; width: 80px; height: 40px; background: #cfc }
          #tr { position: absolute; left: 350px; top: 10px; width: 200px; height: 100px; transform: translateX(10px); background: #eee }
          #tr .fix { position: fixed; left: 50px; top: 150px; width: 150px; height: 60px; background: #ccf }
          #tr .fix .abs { position: absolute; left: 100px; top: 70px; width: 80px; height: 80px; background: #ffc }
          #filt { position: absolute; left: 600px; top: 10px; width: 150px; height: 80px; filter: blur(0); background: #ddd }
          #filt .fix { position: fixed; left: 10px; top: 120px; width: 60px; height: 60px; background: #fdf }
          #wc { position: absolute; left: 10px; top: 300px; width: 100px; height: 60px; will-change: transform; background: #ddd }
          #wc .fix { position: fixed; left: 200px; top: 200px; width: 60px; height: 60px; background: #dfd }
          #plain .fix { position: fixed; right: 0; bottom: 0; width: 100px; height: 50px; background: #fdd }
          #deep { position: absolute; left: 300px; top: 300px; width: 50px; height: 50px; overflow: hidden }
          #deep .abs { position: absolute; left: 200px; top: 150px; width: 100px; height: 60px; background: #cdf }
          #deep .st { position: static }
        </style></head><body>
        <p id=para>Text before <span class=cb>the inline containing block wraps over lines <i>here</i> and
          <span class=abs>abs in inline</span><span class=abs2>abs2</span> continues</span> end.</p>
        <div id=tr><div class=fix>fixed in transformed <div class=abs>abs</div></div></div>
        <div id=filt><div class=fix>fixed in filter</div></div>
        <div id=wc><div class=fix>fixed in will-change</div></div>
        <div id=plain><div class=fix>plain fixed</div></div>
        <div id=deep><div class=st><div class=abs>escapes the static clip ancestor?</div></div></div>
        <div style=\"height:2000px\"></div>
        </body></html>",
    );
    assert!(p.compare("out-of-flow boxes") > 12);
    p.layout.window_scroll_to(&p.dom, None, Some(90));
    assert!(p.compare("out-of-flow boxes scrolled") > 6);
}

#[test]
fn generated_content_markers_columns_and_text_overflow_match_the_scan() {
    let p = page(
        "<!doctype html><html><head><style>
          body { margin: 0; font: 14px/16px sans-serif }
          #gen { position: absolute; left: 10px; top: 10px; width: 200px }
          #gen p::before { content: 'before '; display: block; height: 30px; background: #fcc }
          #gen p::after { content: 'after'; position: absolute; left: 250px; top: 0; width: 80px; height: 80px; background: #cfc }
          #gen p { position: relative; margin: 0 0 10px }
          #list { position: absolute; left: 10px; top: 200px; width: 200px; list-style-position: outside; padding-left: 40px }
          #list li::marker { font-size: 30px }
          #list .in { list-style-position: inside }
          #cols { position: absolute; left: 300px; top: 150px; width: 400px; column-count: 3; column-gap: 10px }
          #cols span { background: #eef }
          #narrow { position: absolute; left: 10px; top: 380px; width: 30px; background: #eee }
          #narrow span { background: #fdc }
          #nowrap { position: absolute; left: 100px; top: 380px; width: 40px; white-space: nowrap; background: #ddd }
          #nowrap b { background: #cdf }
          #ib { position: absolute; left: 300px; top: 380px; width: 400px; line-height: 4px; font-size: 20px }
          #ib .ib { display: inline-block; width: 50px; height: 50px; background: #ffc; vertical-align: middle }
          #ib .ib2 { display: inline-block; line-height: 40px; background: #cff }
          #img { position: absolute; left: 10px; top: 500px }
          #img img { width: 120px; height: 60px; object-fit: none; object-position: -50px 200px; border: 1px solid #888 }
          #pre { position: absolute; left: 300px; top: 520px; width: 60px; white-space: pre; font: 12px/2px monospace }
        </style></head><body>
        <div id=gen><p>first <span>para</span></p><p>second para</p></div>
        <ul id=list><li>one <span>a</span></li><li class=in>two</li><li>three <b>three</b></li></ul>
        <div id=cols>Lorem ipsum dolor sit amet, <span>consectetur adipiscing elit, sed do eiusmod tempor</span> incididunt ut labore et dolore magna aliqua. Ut enim ad minim veniam, quis nostrud exercitation ullamco laboris nisi ut aliquip ex ea commodo consequat. <span>Duis aute irure dolor</span> in reprehenderit.</div>
        <div id=narrow>Supercalifragilisticexpialidocious <span>Antidisestablishmentarianism</span></div>
        <div id=nowrap>no wrap here <b>at all for this long text</b></div>
        <div id=ib>tight <span class=ib></span> line <span class=ib2>block</span> more text <span>span</span> end</div>
        <div id=img><img alt=x src=\"data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='10' height='10'/%3E\"></div>
        <div id=pre>   leading spaces
  <span>tight pre</span>      trailing        </div>
        </body></html>",
    );
    assert!(p.compare("generated content, markers, columns, text") > 15);
}

#[test]
fn slotted_content_outside_its_host_and_the_top_layer_match_the_scan() {
    let mut p = page(
        "<!doctype html><html><head><style>
          body { margin: 0; font: 14px/16px sans-serif }
          #host { position: relative; margin: 100px 0 0 300px; width: 120px; height: 60px; background: #eee }
          .far { position: absolute; left: -280px; top: 150px; width: 200px; height: 80px; background: #fcc }
          .neg { margin-left: -250px; width: 100px; height: 30px; background: #cfc }
          #pop { position: fixed; inset: auto; left: 500px; top: 300px; width: 200px; height: 100px; margin: 0; background: #ffc }
          #pop span { position: absolute; left: -100px; top: 120px; width: 80px; height: 40px; background: #cff }
          #dlg { margin: 0; left: 50px; top: 400px; width: 200px; height: 80px }
          #under { position: absolute; left: 0; top: 0; width: 800px; height: 600px; z-index: -1; opacity: 0.3; background: #000 }
        </style></head><body>
        <div id=host><div class=far id=far>slotted far <b>bold</b></div><div class=neg id=neg>neg</div><span id=txt>text</span></div>
        <div id=under></div>
        <div id=pop popover>popover <span>escapes</span></div>
        <dialog id=dlg>modal dialog <button>ok</button></dialog>
        </body></html>",
    );
    let host = p.id("host");
    let root = p.dom.attach_shadow(host, ShadowRootMode::Open).unwrap();
    fill_shadow(
        &mut p,
        root,
        "<style>.in { transform: translateX(20px) } ::slotted(.neg) { position: relative; top: 40px }</style>
         <div class=in><slot></slot></div>",
    );
    assert!(p.compare("slotted outside host") > 3);
    let pop = p.id("pop");
    p.dom.set_popover_showing(pop, true).unwrap();
    p.layout.style_mut().set_popover_open(pop, true);
    assert!(p.compare("popover showing") > 3);
    let dlg = p.id("dlg");
    p.dom.set_attribute(dlg, "open", "").unwrap();
    p.dom.set_modal(dlg, true).unwrap();
    assert!(p.compare("modal dialog") >= 2);
}

#[test]
fn boxes_moving_between_hit_tests_match_the_scan() {
    let rows: String = (0..30)
        .map(|r| format!("<tr><td>r{r}a</td><td id=c{r}>r{r}b <span>s</span></td><td>r{r}c</td></tr>"))
        .collect();
    let mut p = page(&format!(
        "<!doctype html><html><head><style>
          body {{ margin: 0; font: 13px/15px sans-serif }}
          #t {{ position: absolute; left: 10px; top: 10px; width: 400px; border-collapse: collapse }}
          #t td {{ border: 1px solid #888; height: 14px }}
          #mv {{ position: absolute; left: 500px; top: 20px; width: 80px; height: 80px; background: #fcc }}
          #para {{ position: absolute; left: 450px; top: 300px; width: 300px }}
        </style></head><body>
        <table id=t><tbody id=tb>{rows}</tbody></table>
        <div id=mv>mover <span>s</span></div>
        <p id=para>Some text <span id=tight>that becomes tight</span> later on.</p>
        </body></html>"
    ));
    let carets = caret_grid(&p);
    assert!(p.compare("moving: start") > 20);
    assert_eq!(carets, caret_grid(&p), "carets changed by hit testing alone");
    let mv = p.id("mv");
    let c5 = p.id("c5");
    let c12 = p.id("c12");
    let tight = p.id("tight");
    let tb = p.id("tb");
    let steps: Vec<Box<dyn Fn(&mut Page)>> = vec![
        Box::new(move |p| p.dom.set_attribute(mv, "style", "left: 100px; top: 200px").unwrap()),
        Box::new(move |p| p.dom.set_attribute(mv, "style", "transform: rotate(45deg) translateX(200px)").unwrap()),
        // A cell's content grows outside its row (row_overflow of that row only).
        Box::new(move |p| {
            p.dom.set_attribute(c5, "style", "position: relative").unwrap();
            let d = p.dom.create_element(p.doc, "div").unwrap();
            p.dom
                .set_attribute(
                    d,
                    "style",
                    "position:absolute;left:300px;top:200px;width:90px;height:90px;background:#cfc",
                )
                .unwrap();
            p.dom.append_child(c5, d).unwrap();
        }),
        // ... and moves again, then shrinks back.
        Box::new(move |p| p.dom.set_attribute(c5, "style", "position: relative; left: 200px; top: 300px").unwrap()),
        Box::new(move |p| p.dom.set_attribute(c5, "style", "").unwrap()),
        // A cell becomes transformed without its row changing.
        Box::new(move |p| p.dom.set_attribute(c12, "style", "transform: translate(350px, -100px)").unwrap()),
        // Overflowing content in a cell (no positioning).
        Box::new(move |p| {
            let d = p.dom.create_element(p.doc, "div").unwrap();
            p.dom.set_attribute(d, "style", "width:900px;height:4px;margin-left:-300px;background:#ccf").unwrap();
            p.dom.append_child(c12, d).unwrap();
        }),
        // Rows inserted and removed (splice clears row_overflow).
        Box::new(move |p| {
            for i in 0..5 {
                let tr = p.dom.create_element(p.doc, "tr").unwrap();
                let td = p.dom.create_element(p.doc, "td").unwrap();
                p.dom.set_attribute(td, "colspan", "3").unwrap();
                p.dom.set_attribute(td, "style", "height: 30px; background: #ffc").unwrap();
                let t = p.dom.create_text(p.doc, &format!("new {i}")).unwrap();
                p.dom.append_child(td, t).unwrap();
                p.dom.append_child(tr, td).unwrap();
                let first = p.dom.first_child(tb).unwrap();
                p.dom.insert_before(tb, tr, first).unwrap();
            }
        }),
        Box::new(move |p| {
            let first = p.dom.element_children(tb).unwrap().next().unwrap();
            p.dom.remove_child(tb, first).unwrap();
        }),
        // A tighter line-height (the glyph reach past the lines grows).
        Box::new(move |p| p.dom.set_attribute(tight, "style", "font-size: 40px; line-height: 2px").unwrap()),
        Box::new(move |p| p.dom.set_attribute(tight, "style", "").unwrap()),
        // display: none flips (a box, a row, a row group).
        Box::new(move |p| p.dom.set_attribute(mv, "style", "display: none").unwrap()),
        Box::new(move |p| p.dom.set_attribute(mv, "style", "left: 20px; top: 500px").unwrap()),
        Box::new(move |p| {
            let row = p.dom.parent_element(c12).unwrap().unwrap();
            p.dom.set_attribute(row, "style", "display: none").unwrap();
        }),
        Box::new(move |p| {
            let row = p.dom.parent_element(c12).unwrap().unwrap();
            p.dom.set_attribute(row, "style", "").unwrap();
            p.dom.set_attribute(tb, "style", "visibility: collapse").unwrap();
        }),
        Box::new(move |p| p.dom.set_attribute(tb, "style", "transform: translateX(120px)").unwrap()),
        Box::new(move |p| p.dom.set_attribute(tb, "style", "").unwrap()),
        // Resize between hit tests.
        Box::new(|p| p.layout.set_viewport(Viewport::new(500, 400))),
        Box::new(|p| p.layout.set_viewport(Viewport::new(800, 600))),
        Box::new(|p| {
            p.layout.window_scroll_to(&p.dom, None, Some(60));
        }),
    ];
    for (i, step) in steps.iter().enumerate() {
        step(&mut p);
        let before = caret_grid(&p);
        p.compare(&format!("moving: step {i}"));
        assert_eq!(before, caret_grid(&p), "carets changed by hit testing alone (step {i})");
    }
}

/// A small deterministic generator (LCG).
pub(crate) struct Rng(pub(crate) u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.0 >> 33
    }
    pub(crate) fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    pub(crate) fn pick<'a>(&mut self, xs: &[&'a str]) -> &'a str {
        xs[self.below(xs.len() as u64) as usize]
    }
    pub(crate) fn chance(&mut self, pct: u64) -> bool {
        self.below(100) < pct
    }
}

fn random_style(r: &mut Rng, depth: u32) -> String {
    let mut s = String::new();
    let w = 20 + r.below(260);
    let h = 10 + r.below(160);
    if r.chance(70) {
        s += &format!("width:{w}px;");
    }
    if r.chance(60) {
        s += &format!("height:{h}px;");
    }
    let pos = r.pick(&["static", "static", "relative", "absolute", "absolute", "fixed", "sticky"]);
    s += &format!("position:{pos};");
    if pos != "static" {
        let l = r.below(700) as i64 - 150;
        let t = r.below(560) as i64 - 120;
        s += &format!("left:{l}px;top:{t}px;");
        if r.chance(30) {
            s += &format!("z-index:{};", r.below(7) as i64 - 3);
        }
    }
    if r.chance(25) {
        s += &format!(
            "transform:{};",
            r.pick(&[
                "rotate(23deg)",
                "scale(1.6)",
                "scale(0.5) translate(80px, 40px)",
                "skewX(-30deg)",
                "rotateY(55deg)",
                "perspective(150px) rotateX(40deg)",
                "translate3d(40px, -30px, 50px)",
                "matrix(1, 0.3, -0.4, 1, 30, 10)",
                "rotate(180deg)",
            ])
        );
    }
    if r.chance(25) {
        s += &format!(
            "overflow:{};",
            r.pick(&["hidden", "clip", "auto", "scroll", "visible", "hidden visible", "clip visible", "visible clip"])
        );
    }
    if r.chance(8) {
        s += "contain:paint;";
    }
    if r.chance(12) {
        s += &format!("writing-mode:{};", r.pick(&["vertical-rl", "vertical-lr", "sideways-rl"]));
    }
    if r.chance(10) {
        s += "direction:rtl;";
    }
    if r.chance(20) {
        let m = r.below(200) as i64 - 150;
        s += &format!("margin-{}:{m}px;", r.pick(&["left", "top", "right", "bottom"]));
    }
    if r.chance(15) {
        s += &format!("line-height:{}px;font-size:{}px;", r.below(8), 10 + r.below(30));
    }
    if r.chance(20) {
        s += &format!(
            "display:{};",
            r.pick(&[
                "inline-block",
                "flex",
                "grid",
                "inline",
                "list-item",
                "flow-root",
                "contents",
                "inline-flex",
                "table-cell"
            ])
        );
    }
    if r.chance(6) {
        s += "column-count:2;";
    }
    if r.chance(8) {
        s += &format!("float:{};", r.pick(&["left", "right"]));
    }
    if r.chance(6) {
        s += "pointer-events:none;";
    }
    if r.chance(5) {
        s += "visibility:hidden;";
    }
    if r.chance(6) {
        s += "white-space:nowrap;";
    }
    if r.chance(5) {
        s += "clip-path:inset(10px 20px);";
    }
    if r.chance(10) {
        s += "padding:12px;border:5px solid #888;";
    }
    let _ = depth;
    s += &format!("background:#{:03x};", r.below(0xfff));
    s
}

const WORDS: &[&str] = &["lorem", "ipsum", "dolor", "sit", "amet", "consectetur", "adipiscing", "elit", "sed", "do"];

fn random_text(r: &mut Rng) -> String {
    let n = 1 + r.below(14);
    (0..n).map(|_| r.pick(WORDS)).collect::<Vec<_>>().join(" ")
}

pub(crate) fn random_children(r: &mut Rng, depth: u32, out: &mut String, id: &mut u32) {
    let n = 1 + r.below(if depth == 0 { 9 } else { 4 });
    for _ in 0..n {
        *id += 1;
        let i = *id;
        match r.below(10) {
            0 | 1 => {
                out.push_str(&random_text(r));
                out.push(' ');
            }
            2 => {
                out.push_str(&format!("<span id=e{i} style=\"{}\">{}</span> ", random_style(r, depth), random_text(r)))
            }
            3 if depth < 3 => {
                // A small table.
                let rows = 1 + r.below(4);
                out.push_str(&format!(
                    "<table id=e{i} style=\"{}border-collapse:{}\">",
                    random_style(r, depth),
                    r.pick(&["collapse", "separate"])
                ));
                for row in 0..rows {
                    *id += 1;
                    let tr_style = if r.chance(30) { random_style(r, depth + 1) } else { String::new() };
                    out.push_str(&format!("<tr id=e{} style=\"{tr_style}\">", *id));
                    for _ in 0..1 + r.below(3) {
                        *id += 1;
                        let td_style = if r.chance(40) { random_style(r, depth + 1) } else { String::new() };
                        let span = if row == 0 && r.chance(20) { " rowspan=2" } else { "" };
                        out.push_str(&format!("<td id=e{}{span} style=\"{td_style}\">", *id));
                        if r.chance(50) {
                            random_children(r, depth + 2, out, id);
                        } else {
                            out.push_str(&random_text(r));
                        }
                        out.push_str("</td>");
                    }
                    out.push_str("</tr>");
                }
                out.push_str("</table>");
            }
            4 => out.push_str(&format!(
                "<ul id=e{i} style=\"{}\"><li>{}</li><li>{}</li></ul>",
                random_style(r, depth),
                random_text(r),
                random_text(r)
            )),
            _ if depth < 4 => {
                let tag = r.pick(&["div", "div", "section", "p", "a", "button", "b"]);
                let class = r.pick(&["", "", "pb", "pa"]);
                out.push_str(&format!("<{tag} id=e{i} class=\"{class}\" style=\"{}\">", random_style(r, depth)));
                random_children(r, depth + 1, out, id);
                out.push_str(&format!("</{tag}>"));
            }
            _ => out.push_str(&format!("<i id=e{i}>{}</i>", random_text(r))),
        }
    }
}

/// [`Page::compare`], recording a failure instead of stopping at it.
fn try_compare(p: &Page, label: &str, failures: &mut Vec<String>) -> usize {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| p.compare(label))) {
        Ok(n) => n,
        Err(e) => {
            let msg = e.downcast_ref::<String>().cloned().unwrap_or_default();
            failures.push(msg.lines().take(3).collect::<Vec<_>>().join(" / "));
            0
        }
    }
}

#[test]
fn random_pages_match_the_scan() {
    let pages: u64 = std::env::var("PYRO_HIT_RANDOM_PAGES").ok().and_then(|v| v.parse().ok()).unwrap_or(12);
    let from: u64 = std::env::var("PYRO_HIT_FROM").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    let mut total = 0;
    let mut failures: Vec<String> = Vec::new();
    let only: Option<u64> = std::env::var("PYRO_HIT_SEED").ok().and_then(|v| v.parse().ok());
    for seed in (from..from + pages).filter(|s| only.is_none_or(|o| o == *s)) {
        let mut r = Rng(0x9e37_79b9_7f4a_7c15 ^ seed.wrapping_mul(0x1234_5678_9abc_def1));
        let mut body = String::new();
        let mut id = 0;
        random_children(&mut r, 0, &mut body, &mut id);
        let html = format!(
            "<!doctype html><html><head><style>
              body {{ margin: 0; font: 13px/15px sans-serif }}
              .pb::before {{ content: 'B'; display: block; width: 40px; height: 20px; background: #f0f }}
              .pa::after {{ content: 'A'; position: absolute; left: 30px; top: 40px; width: 50px; height: 30px; background: #0ff }}
            </style></head><body>{body}<div style=\"height:{}px\"></div></body></html>",
            r.below(2000)
        );
        if std::env::var("PYRO_HIT_DUMP").is_ok() {
            std::fs::write(format!("{}/random-{seed}.html", std::env::temp_dir().display()), &html).unwrap();
        }
        let mut p = page(&html);
        total += try_compare(&p, &format!("random page {seed}"), &mut failures);
        // Scroll every scroll container a little, and the window.
        for i in 1..=id {
            if let Ok(Some(el)) = p.dom.get_element_by_id(p.doc, &format!("e{i}")) {
                p.layout.set_scroll_top(&p.dom, el, 25);
                p.layout.set_scroll_left(&p.dom, el, 15);
            }
        }
        p.layout.window_scroll_to(&p.dom, Some(10), Some(70));
        total += try_compare(&p, &format!("random page {seed} scrolled"), &mut failures);
        // Move one element between hit tests.
        if id > 0 {
            let target = 1 + r.below(id as u64) as u32;
            if let Ok(Some(el)) = p.dom.get_element_by_id(p.doc, &format!("e{target}")) {
                p.dom
                    .set_attribute(
                        el,
                        "style",
                        "position:absolute;left:300px;top:250px;width:150px;height:90px;background:#123",
                    )
                    .unwrap();
                total += try_compare(&p, &format!("random page {seed} moved e{target}"), &mut failures);
            }
        }
    }
    assert!(failures.is_empty(), "{} failing pages:\n{}", failures.len(), failures.join("\n"));
    assert!(only.is_some() || total > pages as usize * 3);
}

#[test]
fn css_tables_and_unusual_table_parts_match_the_scan() {
    let p = page(
        "<!doctype html><html><head><style>
          body { margin: 0; font: 13px/15px sans-serif }
          #ct { display: table; position: absolute; left: 10px; top: 10px; width: 300px; border-spacing: 4px }
          #ct .rg { display: table-row-group }
          #ct .r { display: table-row }
          #ct .r.mv { position: relative; left: 50px; transform: translateY(10px) }
          #ct .c { display: table-cell; border: 1px solid #888; height: 20px }
          #ct .c.wide div { width: 400px; height: 10px; background: #fcc }
          #lone { position: absolute; left: 400px; top: 10px; width: 300px }
          #lone .c { display: table-cell; width: 80px; height: 40px; background: #cfc }
          #lone .r { display: table-row }
          #blk { position: absolute; left: 10px; top: 250px; width: 400px }
          #blk tr.block { display: block; background: #ccf; height: 30px }
          #blk tbody.flex { display: flex; background: #ffc }
          #blk td { border: 1px solid #888 }
          #blk td.none { display: none }
          #blk td.ib { display: inline-block; width: 60px; height: 50px; background: #fcf }
          #cap { position: absolute; left: 450px; top: 250px; width: 300px }
          #cap caption { caption-side: bottom; background: #eef; height: 30px }
          #cap col.c2 { background: #fdd }
        </style></head><body>
        <div id=ct><div class=rg><div class=r><div class=c>a</div><div class='c wide'>b<div></div></div></div><div class='r mv'><div class=c>c</div><div class=c>d <span>s</span></div></div></div></div>
        <div id=lone><div class=c>lone cell</div><span>between</span><div class=r><div class=c>in row</div></div></div>
        <table id=blk><tbody><tr class=block><td>blocked row</td></tr><tr><td class=ib>ib</td><td class=none>none</td><td>x</td></tr></tbody>
          <tbody class=flex><tr><td>flex group</td><td>y</td></tr></tbody></table>
        <table id=cap><caption>caption <b>b</b></caption><colgroup><col><col class=c2></colgroup><tr><td>1</td><td>2</td></tr><tr><td colspan=2>3</td></tr></table>
        </body></html>",
    );
    assert!(p.compare("css tables and unusual table parts") > 10);
}

/// Debug aid: `PYRO_HIT_FILE=page.html` reports the mismatches on that page
/// with each hit's ancestor ids (ignored by default).
#[test]
#[ignore]
fn minimize_file() {
    let Ok(path) = std::env::var("PYRO_HIT_FILE") else { return };
    let html = std::fs::read_to_string(path).unwrap();
    let p = page(&html);
    let chain = |n: Option<NodeId>| {
        let mut out = Vec::new();
        let mut cur = n;
        while let Some(c) = cur {
            out.push(p.describe(Some(c)));
            cur = p.dom.parent_element(c).ok().flatten();
        }
        out.join(" < ")
    };
    let mut n = 0;
    let mut y = 1.0;
    while y < 600.0 {
        let mut x = 1.0;
        while x < 800.0 {
            let (s, d) = (p.layout.element_from_point_scan(&p.dom, x, y), p.layout.element_from_point(&p.dom, x, y));
            if s != d {
                n += 1;
                if n <= 3 {
                    eprintln!("({x},{y}) scan {} | descent {}", chain(s), chain(d));
                }
            }
            x += 17.0;
        }
        y += 13.0;
    }
    for y in (1..600).step_by(9) {
        let y = y as f64;
        let (s, d) =
            (p.layout.element_from_vertical_point_scan(&p.dom, y), p.layout.element_from_vertical_point(&p.dom, y));
        if s != d {
            n += 1;
            eprintln!("vertical {y}: scan {} | descent {}", chain(s), chain(d));
        }
    }
    if let Ok(Some(x)) = p.dom.get_element_by_id(p.doc, "x") {
        let mut cur = Some(x);
        while let Some(c) = cur {
            eprintln!("rect {} {:?}", p.describe(Some(c)), p.layout.bounding_client_rect(&p.dom, c));
            cur = p.dom.parent_element(c).ok().flatten();
        }
    }
    eprintln!("MISMATCHES {n}");
}

/// An element that clips its overflow and is the containing block of an
/// absolutely positioned (or fixed) descendant through something other than
/// `position` (a transform, a filter, `contain: paint`/`layout`): the clip
/// applies to the descendant (Chromium's `elementFromPoint` answers `html`
/// over it), in Blitz's bound and in pyro-dom's clip rule (`for_each_clip`:
/// any containing block of the out-of-flow box, not only a positioned one).
/// Failed on 1fa4c241 (pyro-dom let it escape the clip).
#[test]
fn clipping_containing_blocks_without_position_match_the_scan() {
    for (label, style, child) in [
        ("contain paint", "overflow:hidden;contain:paint", "position:absolute;left:270px;top:-60px"),
        ("transform", "overflow:hidden;transform:translateX(0)", "position:absolute;left:270px;top:-60px"),
        ("transform, fixed", "overflow:hidden;transform:translateX(0)", "position:fixed;left:400px;top:300px"),
        ("contain layout", "overflow:hidden;contain:layout", "position:absolute;left:270px;top:-60px"),
        ("filter", "overflow:hidden;filter:blur(0)", "position:absolute;left:270px;top:-60px"),
        ("clip", "overflow:clip;contain:paint", "position:absolute;left:270px;top:-60px"),
    ] {
        let p = page(&format!(
            "<!doctype html><html><head><style>body {{ margin: 0; font: 13px/15px sans-serif }}</style></head><body>
             <div style=\"margin:100px;width:200px;height:80px;{style}\"><div id=x style=\"{child};width:200px;height:40px;background:red\">X</div></div>
             <div style=\"height:2000px\"></div></body></html>"
        ));
        let x = p.id("x");
        let r = p.layout.bounding_client_rect(&p.dom, x);
        let (cx, cy) = (r.x + r.width / 2.0, r.y + r.height / 2.0);
        assert!(r.width > 0.0, "{label}: laid out");
        assert_ne!(p.layout.element_from_point(&p.dom, cx, cy), Some(x), "{label}: clipped, as in Chromium");
        assert_ne!(p.layout.element_from_point_scan(&p.dom, cx, cy), Some(x), "{label}: clipped in the scan");
        p.compare(label);
    }
}

/// Text overflowing its box horizontally on lines of `line-height: 0`: the
/// lines' overflow rect has no height, so `hit::Descent::bound` (and
/// Blitz's `unite_overflow` in the ancestors) drop it, and the descent culls
/// the box the text overflows from. Chromium and the scan hit the span.
/// Fails on 1fa4c241.
#[test]
fn zero_height_line_overflow_matches_the_scan() {
    for (label, html) in [
        (
            "line-height 0",
            "<div style=\"width:42px;line-height:0;font-size:29px;white-space:nowrap\"><span id=x>consectetur amet sit</span></div>",
        ),
        (
            "line-height 0, nested in a zero-height box",
            "<div style=\"width:42px;height:0\"><div style=\"width:42px;line-height:0;font-size:29px;white-space:nowrap\"><span id=x>consectetur amet sit</span></div></div>",
        ),
    ] {
        let p = page(&format!(
            "<!doctype html><html><head><style>body {{ margin: 0; font: 13px/15px sans-serif }}</style></head><body>
             <div style=\"height:100px\"></div>{html}<div style=\"height:2000px\"></div></body></html>"
        ));
        let x = p.id("x");
        assert_eq!(p.layout.element_from_point_scan(&p.dom, 69.0, 92.0), Some(x), "{label}: the scan hits the span");
        assert_eq!(p.layout.element_from_point(&p.dom, 69.0, 92.0), Some(x), "{label}: the descent hits the span");
        p.compare(label);
    }
}

/// A `<td>` in a `display: flow-root` row of an absolutely positioned
/// table reports a zero-width box (0, -2.5, 0 x 5); the vertical hit test
/// (x ignored) hits it in the scan, the descent misses it (zero-area
/// contributions are dropped from the bounds). Fails on 1fa4c241.
#[test]
fn zero_width_box_in_a_block_row_matches_the_scan() {
    let p = page(
        "<!doctype html><html><head><style>body { margin: 0; font: 13px/15px sans-serif }</style></head><body>
         <table style=\"width:144px;position:absolute;left:55px;top:161px;transform:rotate(180deg);contain:paint;writing-mode:vertical-lr;border-collapse:collapse\">
         <tr style=\"height:107px;position:absolute;left:23px;top:82px;margin-left:-141px;line-height:7px;font-size:10px;display:flow-root;padding:12px;border:5px solid #888\">
         <td id=x style=\"height:115px\">amet adipiscing adipiscing amet elit do dolor amet do dolor</td></tr></table>
         <div style=\"height:900px\"></div></body></html>",
    );
    p.compare("zero-width cell");
}

/// `caret_from_point` (mousedown, `caretPositionFromPoint`) reads only the
/// words of the inline formatting contexts in and around the element it
/// resolves in (`GeometrySource::subtree_words`) while the document's words
/// are not read since the last change: it answers what it answers from the
/// document's words, for every element and point. Inline content, anonymous
/// blocks, a table, generated content, a scrolled box, a fixed box, vertical
/// text, and a shadow tree with slotted content.
#[test]
fn caret_from_point_in_a_subtree_matches_the_document_words() {
    let mut p = page(
        "<!doctype html><html><head><style>
          body { margin: 0; font: 14px/16px sans-serif }
          #sc { height: 60px; overflow: auto; width: 300px }
          #fx { position: fixed; left: 500px; top: 400px; width: 200px }
          #v { writing-mode: vertical-rl; height: 120px; position: absolute; left: 600px; top: 10px }
          .gen::before { content: 'gen '; }
          td { padding: 3px }
        </style></head><body>
        <p id=p1>plain <span id=s1>span <b id=b1>bold</b> more</span> tail <a id=a1 href=#>link</a></p>
        <div id=mix>loose text <div id=blk>block <i id=i1>child</i></div> after <em id=em>emphasis</em></div>
        <table id=t><tr id=r1><td id=c1>one cell</td><td id=c2>two <span id=s2>cells</span></td></tr></table>
        <p id=g class=gen>generated before</p>
        <div id=sc><p id=sp1>scrolled one</p><p id=sp2>scrolled two</p><p id=sp3>scrolled three</p><p id=sp4>four</p></div>
        <div id=fx>fixed <span id=fs>box</span></div>
        <div id=v>vertical <span id=vs>text</span></div>
        <div id=host><span id=sl>slotted text</span> light</div>
        <div style=\"height:900px\"></div>
        </body></html>",
    );
    let host = p.id("host");
    let root = p.dom.attach_shadow(host, ShadowRootMode::Open).unwrap();
    fill_shadow(&mut p, root, "<p id=inner>shadow words <slot></slot> end</p><div>more shadow</div>");
    let sc = p.id("sc");
    p.layout.set_scroll_top(&p.dom, sc, 20);
    p.layout.window_scroll_to(&p.dom, None, Some(15));
    let ids = [
        "p1", "s1", "b1", "a1", "mix", "blk", "i1", "em", "t", "r1", "c1", "c2", "s2", "g", "sc", "sp2", "fx", "fs",
        "v", "vs", "host", "sl",
    ];
    let mut els: Vec<NodeId> = ids.iter().map(|id| p.id(id)).collect();
    els.push(p.dom.body(p.doc).unwrap().unwrap());
    let points: Vec<(f64, f64)> =
        (0..24).flat_map(|i| (0..16).map(move |j| (3.0 + i as f64 * 33.0, 2.0 + j as f64 * 37.0))).collect();
    let carets = |p: &Page| -> Vec<Option<(NodeId, u32, bool)>> {
        els.iter()
            .flat_map(|&el| points.iter().map(move |&(x, y)| (el, x, y)))
            .map(|(el, x, y)| p.layout.caret_from_point(&p.dom, el, x, y, &|_| true))
            .collect()
    };
    // No document words read yet (nothing above reads them): the subtree's.
    let subtree = carets(&p);
    p.layout.text_result(&p.dom);
    let document = carets(&p);
    assert_eq!(subtree.len(), document.len());
    let mut found = 0;
    for (i, (a, b)) in subtree.iter().zip(&document).enumerate() {
        let (el, (x, y)) = (els[i / points.len()], points[i % points.len()]);
        assert_eq!(a, b, "caret in {} at ({x}, {y})", p.describe(Some(el)));
        found += a.is_some() as usize;
    }
    assert!(found > points.len() * 4, "carets found: {found}");
}

/// A positioned inline element among block siblings: Blitz wraps the
/// inline content in an anonymous block, which claims the absolutely
/// positioned boxes the inline element contains. The descent (over DOM
/// children) opens their paths from the anonymous blocks too, else a box
/// placed outside its DOM parent's bound (a small block, a table row) is
/// missed. Random page 103.
#[test]
fn out_of_flow_boxes_of_anonymous_blocks_match_the_scan() {
    for (label, inner) in [
        ("block in inline", "<div style=\"width:50px;height:20px\"><b id=x style=\"position:absolute;left:300px;top:250px;width:150px;height:90px;background:#123\">abs</b></div>"),
        ("inline table", "<table style=\"display:inline;position:sticky;top:0\"><tr><td>c<td id=x style=\"position:absolute;left:300px;top:250px;width:150px;height:90px;background:#123\">abs</td></tr><tr><td><p style=\"width:200px;height:300px;margin:0;background:#34a\">tall</p></td></tr></table>"),
    ] {
        let p = page(&format!(
            "<!doctype html><html><head><style>body {{ margin: 0; font: 13px/15px sans-serif }}</style></head><body>
             <div>block sibling</div>text <span style=\"position:relative\">inline {inner}</span> after
             <div style=\"height:900px\"></div></body></html>"
        ));
        let x = p.id("x");
        assert_eq!(p.layout.element_from_point_scan(&p.dom, 320.0, 280.0), Some(x), "{label}: the scan hits it");
        p.compare(label);
    }
}

/// The glyph reach past the line boxes is measured once per line breaking
/// (`TextLayout::hit_reach`) and relative to each line: aligning the lines
/// again (text-align changed, the lines kept) or justifying them keeps the
/// bound holding every run. Tight lines, hanging white space, rtl.
#[test]
fn realigned_lines_match_the_scan() {
    let mut p = page(
        "<!doctype html><html><head><style>
          body { margin: 0; font: 14px/15px sans-serif }
          .t { width: 260px; line-height: 3px; font-size: 22px; white-space: pre-wrap; margin: 40px 0 0 120px }
        </style></head><body>
        <div id=a class=t><span id=s1>lorem ipsum dolor sit amet</span>    <b id=s2>consectetur adipiscing</b> elit sed do    </div>
        <div id=b class=t dir=rtl><span id=s3>amet dolor sit</span>   <i id=s4>ipsum lorem do</i> elit   </div>
        <div style=\"height:900px\"></div></body></html>",
    );
    p.compare("start");
    for align in ["center", "right", "justify", "left", "end", "justify;text-align-last:justify", "center"] {
        for id in ["a", "b"] {
            let el = p.id(id);
            p.dom.set_attribute(el, "style", &format!("text-align:{align}")).unwrap();
        }
        p.compare(align);
    }
}

// ---- review (lay2): hit bounds are brought up to date at the first hit test after a resolve
// (`BaseDocument::ensure_hit_overflow`), not in every transform pass. Hit tests interleaved with
// mutations, forced layouts, scrolling and viewport changes in every order. ----

/// The descent against the scan at a coarse grid and some random points; the descent asked first,
/// straight after whatever came before (so it is the query that syncs the layout when one is due).
fn lay2_compare_points(p: &Page, r: &mut Rng, label: &str, failures: &mut Vec<String>) {
    let mut points = vec![(r.below(800) as f64 + 0.5, r.below(600) as f64)];
    for y in (5..600).step_by(43) {
        for x in (5..800).step_by(37) {
            points.push((x as f64, y as f64));
        }
    }
    for _ in 0..40 {
        points.push((r.below(1600) as f64 / 2.0, r.below(1200) as f64 / 2.0));
    }
    // Whether this round's layout is indexed at once or left to the tests
    // (a descent first, the index when the descents have paid for one):
    // either way the index of the layout before the mutations is gone.
    let force = r.chance(50);
    for (i, (x, y)) in points.into_iter().enumerate() {
        let asked = match force && i == 0 {
            true => p.layout.element_from_point_indexed(&p.dom, x, y),
            false => p.layout.element_from_point(&p.dom, x, y),
        };
        let scan = p.layout.element_from_point_scan(&p.dom, x, y);
        let descent = p.layout.element_from_point_descent(&p.dom, x, y);
        let indexed = match i % 3 {
            0 => p.layout.element_from_point_indexed(&p.dom, x, y),
            _ => scan,
        };
        if scan != descent || scan != asked || scan != indexed {
            failures.push(format!(
                "{label} at ({x}, {y}): scan {} descent {} asked {} index {}",
                p.describe(scan),
                p.describe(descent),
                p.describe(asked),
                p.describe(indexed)
            ));
            return;
        }
    }
}

#[test]
fn random_pages_with_hit_tests_interleaved_with_mutations_match_the_scan() {
    let env = |k: &str| std::env::var(k).ok().and_then(|v| v.parse::<u64>().ok());
    let pages = env("PYRO_HIT_LAY2_PAGES").unwrap_or(if cfg!(debug_assertions) { 6 } else { 40 });
    let from = env("PYRO_HIT_LAY2_FROM").unwrap_or(0);
    let rounds = env("PYRO_HIT_LAY2_ROUNDS").unwrap_or(14);
    let mut failures: Vec<String> = Vec::new();
    for seed in from..from + pages {
        let mut r = Rng(0x51ed_270b_27a4_c3f9 ^ seed.wrapping_mul(0x2545_f491_4f6c_dd1d));
        let mut body = String::new();
        let mut id = 0;
        random_children(&mut r, 0, &mut body, &mut id);
        let html = format!(
            "<!doctype html><html><head><style>
              body {{ margin: 0; font: 13px/15px sans-serif }}
              .pb::before {{ content: 'B'; display: block; width: 40px; height: 20px; background: #f0f }}
              .pa::after {{ content: 'A'; position: absolute; left: 30px; top: 40px; width: 50px; height: 30px; background: #0ff }}
              .wide {{ width: 700px !important }} .gone {{ display: none }} .far {{ margin-left: 300px }}
            </style></head><body>{body}<div style=\"height:{}px\"></div></body></html>",
            r.below(2000)
        );
        let mut p = page(&html);
        let el = |p: &Page, r: &mut Rng| -> Option<NodeId> {
            if id == 0 {
                return None;
            }
            p.dom.get_element_by_id(p.doc, &format!("e{}", 1 + r.below(id as u64))).ok().flatten()
        };
        let mut log: Vec<String> = Vec::new();
        let before = failures.len();
        for round in 0..rounds {
            // One to three mutations, with forced layouts (no hit test) between some of them.
            for _ in 0..1 + r.below(3) {
                let Some(e) = el(&p, &mut r) else { break };
                match r.below(7) {
                    0 => {
                        let s = random_style(&mut r, 1);
                        p.dom.set_attribute(e, "style", &s).unwrap();
                        log.push(format!("style e: {s}"));
                    }
                    1 => {
                        let t = random_text(&mut r);
                        p.dom.set_text_content(e, &t).unwrap();
                        log.push("text".into());
                    }
                    2 => {
                        let c = r.pick(&["wide", "gone", "far", "pb", "pa", ""]);
                        p.dom.set_attribute(e, "class", c).unwrap();
                        log.push(format!("class {c}"));
                    }
                    3 => {
                        let x = p.dom.create_element(p.doc, "div").unwrap();
                        p.dom.set_attribute(x, "style", &random_style(&mut r, 2)).unwrap();
                        let t = p.dom.create_text(p.doc, &random_text(&mut r)).unwrap();
                        p.dom.append_child(x, t).unwrap();
                        if p.dom.append_child(e, x).is_err() {
                            continue;
                        }
                        log.push("insert".into());
                    }
                    4 => {
                        if let Ok(Some(parent)) = p.dom.parent(e) {
                            p.dom.remove_child(parent, e).unwrap();
                            log.push("remove".into());
                        }
                    }
                    5 => {
                        // A forced layout with no hit test after it.
                        let _ = p.layout.bounding_client_rect(&p.dom, e);
                        log.push("rect".into());
                    }
                    _ => {
                        p.layout.set_scroll_top(&p.dom, e, r.below(60) as i32);
                        p.layout.set_scroll_left(&p.dom, e, r.below(40) as i32);
                        log.push("scroll element".into());
                    }
                }
            }
            match r.below(6) {
                0 => {
                    p.layout.window_scroll_to(&p.dom, Some(r.below(30) as i32), Some(r.below(200) as i32));
                    log.push("window scroll".into());
                }
                1 => {
                    // A forced layout, then the hit test.
                    let body = p.dom.body(p.doc).ok().flatten().unwrap_or(p.doc);
                    let _ = p.layout.offset_height(&p.dom, body);
                }
                2 if round % 5 == 4 => {
                    let w = 640 + r.below(320) as u32;
                    p.layout.set_viewport(Viewport::new(w, 600));
                    log.push(format!("viewport {w}"));
                }
                _ => {}
            }
            // Hit tests only after some rounds: several resolves between them.
            if r.below(3) != 0 {
                lay2_compare_points(&p, &mut r, &format!("seed {seed} round {round}"), &mut failures);
                if failures.len() > before {
                    let tail = &log[log.len().saturating_sub(10)..];
                    let last = failures.pop().unwrap();
                    failures.push(format!("{last} after {tail:?}"));
                    break;
                }
            }
        }
    }
    assert!(failures.is_empty(), "{} failing seeds:\n{}", failures.len(), failures.join("\n"));
}
