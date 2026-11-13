//! `read: "aria"`: the ARIA tree Playwright's `ariaSnapshot()` renders.
use super::*;

const PAGE: &str = r#"<!doctype html><body>
<nav aria-label="Main"><ul><li><a href="/home">Home</a></li></ul></nav>
<h2>Basket</h2>
<p>Two <b>items</b></p>
<label>Coupon <input placeholder="CODE" value="SPRING"></label>
<input type="checkbox" checked aria-label="Gift">
<button disabled>Pay</button><button aria-pressed="mixed">Fav</button>
<details open><summary>More</summary>body</details>
<div hidden>secret</div><span aria-hidden="true">decor</span><div role="none">flat</div>
</body>"#;

#[test]
fn aria_read_returns_playwrights_tree() {
    let mut h = Harness::new();
    load_html(&mut h, PAGE);
    let r = h.page("Pyro.query", json!({"locator": {"css": "body"}, "read": "aria"}));
    let tree = &r["result"]["value"];
    assert_eq!(tree["role"], "fragment", "{r}");
    let kids = tree["children"].as_array().expect("children");
    let nav = &kids[0];
    assert_eq!(nav["role"], "navigation");
    assert_eq!(nav["name"], "Main");
    let link = &nav["children"][0]["children"][0]["children"][0];
    assert_eq!(link["role"], "link", "{nav}");
    assert_eq!(link["name"], "Home");
    assert_eq!(link["props"]["url"], "/home");
    assert_eq!(link["children"], json!([]), "a lone child equal to the name is dropped");
    let find = |role: &str| kids.iter().find(|k| k["role"] == role).cloned().unwrap_or(Value::Null);
    assert_eq!(find("heading")["level"], 2);
    assert_eq!(find("paragraph")["children"], json!(["Two items"]));
    let textbox = find("textbox");
    assert_eq!(textbox["name"], "Coupon");
    assert_eq!(textbox["props"]["placeholder"], "CODE");
    assert_eq!(textbox["children"], json!(["SPRING"]));
    assert_eq!(find("checkbox")["checked"], true);
    let buttons: Vec<&Value> = kids.iter().filter(|k| k["role"] == "button").collect();
    assert_eq!(buttons[0]["disabled"], true);
    assert_eq!(buttons[1]["pressed"], "mixed");
    assert!(find("group").get("expanded").is_none(), "details is a group, not an expanded role: {tree}");
    let text = serde_json::to_string(tree).unwrap();
    assert!(!text.contains("secret") && !text.contains("decor"), "{text}");
    assert!(kids.iter().any(|k| k == "flat"), "role=none content joins its parent: {text}");
}

fn aria(h: &mut Harness, css: &str) -> Value {
    let r = h.page("Pyro.query", json!({"locator": {"css": css}, "read": "aria"}));
    r["result"]["value"].clone()
}

#[test]
fn aria_read_walks_as_playwright() {
    let mut h = Harness::new();
    load_html(
        &mut h,
        r#"<!doctype html><body>
<select id="s" aria-label="Size"><option>S</option><option selected>M</option><optgroup label="G"><option>L</option></optgroup></select>
<div aria-hidden="true"><nav aria-label="Hidden"><a href="/x">x</a></nav></div>
<div id="owner" role="list" aria-owns="two c1"><div role="listitem">one</div><div role="listitem" id="two">two</div></div>
<div id="c1" aria-owns="c2"><div id="c2" aria-owns="c1">loop</div></div>
<div id="roles"><div role="foo button">Act</div><div role="Button">plain</div><button role="none">Go</button>
<h3 role="presentation" aria-describedby="roles">T</h3><div role="heading" aria-level="3.0">L</div><div role="generic"></div></div>
</body>"#,
    );
    let select = aria(&mut h, "#s");
    let combo = &select["children"][0];
    assert_eq!(combo["role"], "combobox", "{select}");
    let opts: Vec<&Value> = combo["children"].as_array().unwrap().iter().collect();
    assert_eq!(opts.len(), 2, "options shown, the drop-down's optgroup is not: {select}");
    assert_eq!(opts[1]["name"], "M");
    assert_eq!(opts[1]["selected"], true);

    assert_eq!(aria(&mut h, "nav")["children"], json!([]), "a root inside aria-hidden is empty");

    let owner = aria(&mut h, "#owner");
    let text = serde_json::to_string(&owner).unwrap();
    assert_eq!(text.matches("\"two\"").count(), 1, "an owned child is shown once: {text}");
    assert_eq!(text.matches("loop").count(), 1, "an ownership cycle ends: {text}");

    let roles = aria(&mut h, "#roles");
    let kids = roles["children"].as_array().unwrap();
    assert_eq!(kids[0]["role"], "button", "first valid token: {roles}");
    assert_eq!(kids[1], "plain", "roles are case-sensitive: {roles}");
    assert_eq!(kids[2]["role"], "button", "none yields to focus: {roles}");
    assert_eq!(kids[3]["role"], "heading", "presentation yields to a global aria-*: {roles}");
    assert_eq!(kids[4]["level"], 3, "aria-level is read as Number(): {roles}");
    assert_eq!(kids.len(), 5, "an empty unnamed generic is dropped: {roles}");
}

#[test]
fn aria_read_hides_unslotted_light_dom() {
    let mut h = Harness::new();
    load_html(
        &mut h,
        r#"<!doctype html><body><div id="host"><b>light</b><i slot="s">slotted</i></div>
<script>host.attachShadow({ mode: 'open' }).innerHTML = '<p>shadow</p><slot name="s"></slot>';</script></body>"#,
    );
    let tree = aria(&mut h, "#host");
    let text = serde_json::to_string(&tree).unwrap();
    assert!(!text.contains("light"), "{text}");
    assert!(text.contains("shadow") && text.contains("slotted"), "{text}");
}

#[test]
fn options_compute_as_in_chromium() {
    let mut h = Harness::new();
    load_html(
        &mut h,
        r#"<!doctype html><body>
<select id="s"><option id="o1">a</option><optgroup id="g" label="G"><option id="o2">b</option></optgroup><option id="oh" hidden>h</option><option id="oc" style="display:none">c</option></select>
<select id="lb" multiple><option id="o3">d</option></select>
<option id="loose">loose</option>
</body>"#,
    );
    let probe = |h: &mut Harness, id: &str| {
        eval_value(
            h,
            &format!(
                "(() => {{ const e = document.getElementById('{id}'); \
                 return [getComputedStyle(e).display, e.checkVisibility(), e.getBoundingClientRect().height > 0]; }})()"
            ),
        )
    };
    assert_eq!(probe(&mut h, "loose"), json!(["block", true, true]), "a loose option is a block box");
    assert_eq!(probe(&mut h, "o1"), json!(["block", false, false]), "a drop-down's option: block, no box");
    assert_eq!(probe(&mut h, "g"), json!(["block", false, false]));
    assert_eq!(probe(&mut h, "o2"), json!(["block", false, false]));
    assert_eq!(probe(&mut h, "oh"), json!(["none", false, false]));
    assert_eq!(probe(&mut h, "o3"), json!(["block", true, true]), "a list box lays its options out");
    // Playwright's getByRole: an option inside a select skips the box check,
    // but its own display:none (hidden, author CSS) still hides it.
    let r = h.page("Pyro.query", json!({"locator": {"role": "option"}, "read": "count"}));
    assert_eq!(r["result"]["value"], 4, "o1, o2, o3 and the loose option: {r}");
    let loose = aria(&mut h, "#loose");
    assert_eq!(loose["children"][0]["role"], "option", "{loose}");
    assert_eq!(loose["children"][0]["name"], "loose", "{loose}");
    // A drop-down turned list box at run time lays its options' text out.
    let w = eval_value(
        &mut h,
        "(() => { s.setAttribute('multiple', ''); document.body.offsetWidth;          const r = o1.getBoundingClientRect(); return [r.width > 6, r.height > 0, getComputedStyle(o2).fontWeight]; })()",
    );
    assert_eq!(w, json!([true, true, "400"]), "options get text boxes; an option is not bold in its optgroup");
}

#[test]
fn generated_content_names_and_snapshots() {
    let mut h = Harness::new();
    load_html(
        &mut h,
        r#"<!doctype html><head><style>
#a::before { content: "Hello " } #a::after { content: attr(data-x) }
#b::before { content: "\2605" / "star"; display: block }
#c::before { content: counter(n) "x" }
#d::before { content: "gone"; visibility: hidden }
#e::before { content: "gone"; display: none }
#p::before { content: "Note: " } #p::after { content: "!" }
#f::before { content: var(--unset) }
</style></head><body>
<button id="a" data-x="!">World</button><button id="b">Go</button><button id="c">Count</button>
<button id="d">Vis</button><button id="e">Disp</button><button id="f">Var</button>
<p id="p">Text</p>
</body>"#,
    );
    let name = |h: &mut Harness, css: &str| aria(h, css)["children"][0]["name"].clone();
    assert_eq!(name(&mut h, "#a"), "Hello World!");
    assert_eq!(name(&mut h, "#b"), "star Go", "alt text, padded as a block");
    assert_eq!(name(&mut h, "#c"), "Count", "a counter is content Playwright does not read");
    assert_eq!(name(&mut h, "#d"), "Vis");
    assert_eq!(name(&mut h, "#e"), "Disp");
    assert_eq!(name(&mut h, "#f"), "Var");
    assert_eq!(aria(&mut h, "#p")["children"][0]["children"], json!(["Note: Text!"]));
    let r = h.page(
        "Pyro.query",
        json!({"locator": {"role": "button", "name": "Hello World!", "exact": true}, "read": "count"}),
    );
    assert_eq!(r["result"]["value"], 1, "{r}");
    assert_eq!(
        eval_value(
            &mut h,
            "[getComputedStyle(a, '::before').content, getComputedStyle(b, ':before').display, \
              getComputedStyle(p, '::after').getPropertyValue('content'), getComputedStyle(c).content, \
              getComputedStyle(d, '::after').content, getComputedStyle(e, '::before').display, \
              getComputedStyle(a, '::bogus').content, getComputedStyle(a, 'before').content]"
        ),
        json!(["\"Hello \"", "block", "\"!\"", "normal", "none", "none", "", "normal"])
    );
}

fn run_read(h: &mut Harness, read: Value) -> Value {
    let r = h.page("Pyro.run", json!({"steps": [{"read": read}]}));
    assert!(r.get("error").is_none(), "{r}");
    r["result"]["reads"]["v"].clone()
}

/// The refs of an AI tree, in order.
fn refs(tree: &Value, out: &mut Vec<String>) {
    if let Some(r) = tree["ref"].as_str() {
        out.push(r.to_owned());
    }
    for c in tree["children"].as_array().into_iter().flatten() {
        refs(c, out);
    }
}

#[test]
fn aria_ai_mode_gives_refs_and_aria_ref_finds_them() {
    let mut h = Harness::new();
    load_html(
        &mut h,
        r#"<!doctype html><body>
<div><div><button id="b">Buy</button></div></div>
<span aria-hidden="true">decor</span><div hidden>secret</div>
<div style="cursor:pointer"><span>Card</span> <b>x</b></div>
<p style="pointer-events:none">np <button>Inert</button></p>
</body>"#,
    );
    let read = json!({"locator": {"css": "body"}, "read": "aria", "mode": "ai", "as": "v"});
    let tree = run_read(&mut h, read.clone());
    let body = &tree["children"][0];
    assert_eq!(body["role"], "generic", "{tree}");
    assert_eq!(body["active"], true, "the body is document.activeElement: {tree}");
    let kids = body["children"].as_array().unwrap();
    assert_eq!(kids[0]["role"], "button", "a generic with one ref'd child gives way to it: {tree}");
    assert_eq!(kids[1], "decor", "aria-hidden but visible text stays: {tree}");
    assert_eq!(kids[2]["cursor"], "pointer", "{tree}");
    assert!(kids[3].get("ref").is_none() && kids[3]["children"][1].get("ref").is_none(), "pointer-events:none: {tree}");
    assert!(!serde_json::to_string(&tree).unwrap().contains("secret"));
    let mut first = Vec::new();
    refs(&tree, &mut first);
    assert_eq!(first, ["e1", "e4", "e5"], "{tree}");

    // Refs stay with their elements across snapshots.
    let mut again = Vec::new();
    refs(&run_read(&mut h, read.clone()), &mut again);
    assert_eq!(first, again);
    let count = |h: &mut Harness, r: &str| {
        h.page("Pyro.query", json!({"locator": {"ariaRef": r}, "read": "count"}))["result"]["value"].clone()
    };
    assert_eq!(count(&mut h, "e4"), 1);
    assert_eq!(count(&mut h, "e99"), 0);
    let id = run_read(&mut h, json!({"locator": {"ariaRef": "e4"}, "read": "name", "as": "v"}));
    assert_eq!(id, "Buy");
    // A removed element is not found any more.
    eval_value(&mut h, "document.getElementById('b').remove()");
    assert_eq!(count(&mut h, "e4"), 0);
    // Pyro.query takes the mode too, and rejects an unknown one.
    let q = h.page("Pyro.query", json!({"locator": {"css": "body"}, "read": "aria", "mode": "ai"}));
    assert_eq!(q["result"]["value"]["children"][0]["ref"], "e1", "{q}");
    let bad = h.page("Pyro.query", json!({"locator": {"css": "body"}, "read": "aria", "mode": "codegen"}));
    assert!(bad.get("error").is_some(), "{bad}");
}

/// A file input is a button named by its labels, else "Choose File", as
/// Playwright's roleUtils has it: agents need its ref to open the chooser.
#[test]
fn file_inputs_are_buttons_with_refs() {
    let mut h = Harness::new();
    load_html(
        &mut h,
        r#"<!doctype html><body><input type="file" id="a"><label>Avatar <input type="file" id="b"></label></body>"#,
    );
    let tree = run_read(&mut h, json!({"locator": {"css": "body"}, "read": "aria", "mode": "ai", "as": "v"}));
    let text = serde_json::to_string(&tree).unwrap();
    fn walk(v: &Value, out: &mut Vec<(String, String)>) {
        if v["role"] == "button" {
            out.push((v["name"].as_str().unwrap_or("").to_owned(), v["ref"].as_str().unwrap_or("").to_owned()));
        }
        for c in v["children"].as_array().into_iter().flatten() {
            walk(c, out);
        }
    }
    let mut found = Vec::new();
    walk(&tree, &mut found);
    let names: Vec<&str> = found.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, ["Choose File", "Avatar"], "{text}");
    assert!(found.iter().all(|(_, r)| r.starts_with('e')), "every file input has a ref: {text}");
    let r = h.page("Pyro.query", json!({"locator": {"role": "button", "name": "Avatar"}, "read": "count"}));
    assert_eq!(r["result"]["value"], 1, "{r}");
}

#[test]
fn aria_ai_mode_names_frame_refs_by_seq() {
    let mut h = Harness::new();
    load_html(&mut h, r#"<!doctype html><body><p>top</p><iframe srcdoc="<button>Inner</button>"></iframe></body>"#);
    let r = h.page(
        "Pyro.run",
        json!({"steps": [{"check": {"locator": {"role": "button", "frame": {"css": "iframe"}}, "assert": "visible"}}]}),
    );
    assert!(r.get("error").is_none(), "{r}");
    let top = run_read(&mut h, json!({"locator": {"css": "body"}, "read": "aria", "mode": "ai", "as": "v"}));
    let iframe = &top["iframes"][0];
    let (iref, frame_id) = (iframe["ref"].as_str().unwrap().to_owned(), iframe["frameId"].as_str().unwrap().to_owned());
    assert_eq!(top["children"][0]["children"][1]["role"], "iframe", "{top}");
    assert_eq!(top["children"][0]["children"][1]["ref"], iref.as_str());
    let child = run_read(
        &mut h,
        json!({"locator": {"css": "body", "frame": {"ariaRef": iref}}, "read": "aria", "mode": "ai",
               "frameSeqs": {frame_id: 3}, "as": "v"}),
    );
    let mut got = Vec::new();
    refs(&child, &mut got);
    // The body (f3e1) gives way to its one ref'd child.
    assert_eq!(got, ["f3e2"], "{child}");
    // A frame ref is looked up in its frame from the page.
    let name = run_read(&mut h, json!({"locator": {"ariaRef": "f3e2"}, "read": "name", "as": "v"}));
    assert_eq!(name, "Inner");
}

#[test]
fn aria_ai_mode_marks_the_shadow_host_active() {
    let mut h = Harness::new();
    load_html(
        &mut h,
        r#"<!doctype html><body><p>top</p><div id="host"></div>
<script>host.attachShadow({ mode: 'open' }).innerHTML = '<button id="in">Inner</button> tail'; host.shadowRoot.getElementById('in').focus();</script></body>"#,
    );
    // document.activeElement is the host, so the host is [active], not the button.
    assert_eq!(eval_value(&mut h, "document.activeElement.id"), "host");
    assert_eq!(eval_value(&mut h, "host.shadowRoot.activeElement.id"), "in");
    let tree = run_read(&mut h, json!({"locator": {"css": "body"}, "read": "aria", "mode": "ai", "as": "v"}));
    let s = serde_json::to_string(&tree).unwrap();
    assert_eq!(s.matches("\"active\":true").count(), 1, "{s}");
    let body = &tree["children"][0];
    assert!(body.get("active").is_none(), "{tree}");
    let host = &body["children"][1];
    assert_eq!(host["active"], true, "{tree}");
    assert!(host["children"][0].get("active").is_none(), "{tree}");
}

#[test]
fn list_boxes_are_size_rows_tall() {
    let mut h = Harness::new();
    load_html(
        &mut h,
        r#"<!doctype html><body>
<select id="a" size="3"><option>A</option><option>B</option><option>C</option><option>D</option><option>E</option></select>
<select id="b" multiple><option>A</option><option>B</option><option>C</option><option>D</option><option>E</option><option>F</option></select>
<select id="c" size="6"><option>A</option><option>B</option></select>
<select id="d" size="2"></select>
<select id="e" size="3" style="font-size:20px"><option>A</option><option>B</option><option>C</option><option>D</option></select>
<select id="f" size="3" style="height:30px"><option>A</option><option>B</option><option>C</option><option>D</option></select>
<select id="g" size="2"><optgroup label="G"><option>A</option></optgroup><option>B</option><option>C</option></select>
<select id="h" size="3" style="padding:5px"><option style="padding:6px">A</option><option>B</option><option>C</option><option>D</option></select>
</body>"#,
    );
    // Chromium's heights: `size` rows (4 for multiple) of the first option,
    // however many options there are, plus padding and border.
    let heights = eval_value(
        &mut h,
        "[...document.querySelectorAll('select')].map(e => [e.id, e.getBoundingClientRect().height, e.clientHeight])",
    );
    assert_eq!(
        heights,
        json!([
            ["a", 53, 51],
            ["b", 70, 68],
            ["c", 104, 102],
            ["d", 34, 32],
            ["e", 77, 75],
            ["f", 30, 28],
            ["g", 36, 34],
            ["h", 96, 94]
        ])
    );
}

#[test]
fn list_box_optgroups_have_a_label_row() {
    let mut h = Harness::new();
    load_html(
        &mut h,
        r#"<!doctype html><body>
<select id="g" size="2"><optgroup label="G"><option>A</option></optgroup><option>B</option><option>C</option></select>
<select id="i" size="6"><optgroup label="G1"><option>A</option><option>B</option></optgroup><optgroup label="G2"><option>C</option></optgroup></select>
<select id="j" size="4"><optgroup label=""></optgroup><option>A</option></select>
<select id="k" multiple style="font-size:20px"><optgroup label="G" style="padding:4px"><option>A</option></optgroup></select>
</body>"#,
    );
    // Chromium's boxes: a label row as tall as an option, then the group's
    // options, indented 20px.
    let boxes = eval_value(
        &mut h,
        "[...document.querySelectorAll('select')].map(s => { const t = s.getBoundingClientRect().y; \
         return [s.id, s.scrollHeight, ...[...s.querySelectorAll('optgroup,option')].map(e => { const r = e.getBoundingClientRect(); return [r.y - t, r.height]; })]; })",
    );
    assert_eq!(
        boxes,
        json!([
            ["g", 68, [1, 34], [18, 17], [35, 17], [52, 17]],
            ["i", 102, [1, 51], [18, 17], [35, 17], [52, 34], [69, 17]],
            ["j", 68, [1, 17], [18, 17]],
            ["k", 100, [1, 58], [30, 25]]
        ])
    );
    let indent = eval_value(
        &mut h,
        "[...document.querySelectorAll('#g option')].map(o => getComputedStyle(o).paddingInlineStart).concat(getComputedStyle(document.querySelector('optgroup'), '::before').content)",
    );
    assert_eq!(indent, json!(["20px", "2px", "2px", "none"]));
}

#[test]
fn selects_sit_on_the_baseline_as_in_chromium() {
    let mut h = Harness::new();
    load_html(
        &mut h,
        r#"<!doctype html><body><div style="font-size:16px"><span id="b" style="display:inline-block;width:1px;height:0"></span><select><option>A</option></select><select style="padding:3px"><option>A</option></select><select style="font-size:20px"><option>A</option></select><select size="3"><option>A</option></select><select size="3" style="margin:5px 0 9px"><optgroup label="G"><option>A</option></optgroup></select></div></body>"#,
    );
    // A drop-down's baseline is its text's; a list box is `vertical-align:
    // text-bottom`. [top, bottom] relative to the line's baseline.
    let boxes = eval_value(
        &mut h,
        "(() => { const b = document.getElementById('b').getBoundingClientRect().y; \
         return [...document.querySelectorAll('select')].map(s => { const r = s.getBoundingClientRect(); return [r.y - b, r.bottom - b]; }); })()",
    );
    assert_eq!(boxes, json!([[-14, 5], [-17, 8], [-20, 6], [-49, 4], [-58, -5]]));
    let va = eval_value(&mut h, "[...document.querySelectorAll('select')].map(s => getComputedStyle(s).verticalAlign)");
    assert_eq!(va, json!(["baseline", "baseline", "baseline", "text-bottom", "text-bottom"]));
}
