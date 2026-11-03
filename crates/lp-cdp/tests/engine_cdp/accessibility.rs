//! Accessibility domain: Chrome-shaped AX trees from the driver's role and
//! accessible-name computation, with backend node ids shared with DOM.
use super::*;

const PAGE: &str = r#"<!doctype html><title>AX page</title><body>
<h1>Hello <b>world</b></h1>
<div><span>plain</span></div>
<div><div><p>nested</p></div></div>
<label for="name">Name</label><input id="name" required value="v">
<input type="checkbox" checked aria-label="Agree">
<ul><li>one</li></ul>
<div aria-hidden="true"><button>hidden</button></div>
<div style="display:none"><button>gone</button></div>
<a href="https://example.com/x">link</a>
<select aria-label="pick"><option>A</option><option selected>B</option></select>
<button id="go" disabled title="tip">Go</button>
<input type="range" value="5" aria-label="volume">
</body>"#;

fn nodes(r: &Value) -> Vec<Value> {
    r["result"]["nodes"].as_array().unwrap_or_else(|| panic!("{r}")).clone()
}

fn by_id(nodes: &[Value]) -> std::collections::HashMap<String, Value> {
    nodes.iter().map(|n| (n["nodeId"].as_str().unwrap().to_owned(), n.clone())).collect()
}

/// `role "name"` lines, indented by depth, unignored nodes only flattened
/// through ignored ones.
fn outline(nodes: &[Value]) -> Vec<String> {
    let map = by_id(nodes);
    let mut out = Vec::new();
    fn walk(map: &std::collections::HashMap<String, Value>, id: &str, depth: usize, out: &mut Vec<String>) {
        let Some(n) = map.get(id) else { return };
        let mut depth = depth;
        if n["ignored"] == false {
            out.push(format!("{}{} {}", "  ".repeat(depth), n["role"]["value"].as_str().unwrap(), n["name"]["value"]));
            depth += 1;
        }
        for c in n["childIds"].as_array().unwrap() {
            walk(map, c.as_str().unwrap(), depth, out);
        }
    }
    walk(&map, nodes[0]["nodeId"].as_str().unwrap(), 0, &mut out);
    out
}

fn prop<'a>(n: &'a Value, name: &str) -> &'a Value {
    n["properties"].as_array().and_then(|p| p.iter().find(|p| p["name"] == name)).map_or(&Value::Null, |p| &p["value"])
}

fn object(h: &mut Harness, selector: &str) -> String {
    let r = h.page("Runtime.evaluate", json!({"expression": format!("document.querySelector({})", json!(selector))}));
    r["result"]["result"]["objectId"].as_str().unwrap_or_else(|| panic!("{selector}: {r}")).to_owned()
}

#[test]
fn full_ax_tree_matches_chrome_structure() {
    let mut h = Harness::new();
    load_html(&mut h, PAGE);
    assert_eq!(h.page("Accessibility.enable", json!({}))["result"], json!({}));
    let r = h.page("Accessibility.getFullAXTree", json!({}));
    let nodes = nodes(&r);
    let root = &nodes[0];
    assert_eq!(root["role"], json!({"type":"internalRole","value":"RootWebArea"}), "{root}");
    assert_eq!(root["name"]["value"], "AX page");
    assert_eq!(root["frameId"], json!(h.frame_id), "{root}");
    assert!(root.get("parentId").is_none());
    assert_eq!(prop(root, "focusable")["value"], true);
    // `<html>` and `<body>` are ignored, layout-only.
    let html = &by_id(&nodes)[root["childIds"][0].as_str().unwrap()];
    assert_eq!(html["ignored"], true, "{html}");
    assert_eq!(html["ignoredReasons"][0]["name"], "uninteresting");
    assert_eq!(html["role"]["value"], "none");
    assert!(html.get("name").is_none() && html.get("properties").is_none(), "{html}");
    // Chrome 1:1 outline of the unignored nodes (minus InlineTextBox,
    // ListMarker and select popup/inner-editor nodes).
    assert_eq!(
        outline(&nodes),
        [
            "RootWebArea \"AX page\"",
            "  heading \"Hello world\"",
            "    StaticText \"Hello \"",
            "    StaticText \"world\"",
            "  generic \"\"",
            "    StaticText \"plain\"",
            "  paragraph \"\"",
            "    StaticText \"nested\"",
            "  LabelText \"\"",
            "    StaticText \"Name\"",
            "  textbox \"Name\"",
            "  checkbox \"Agree\"",
            "  list \"\"",
            "    listitem \"\"",
            "      StaticText \"one\"",
            "  link \"link\"",
            "    StaticText \"link\"",
            "  combobox \"pick\"",
            "    option \"A\"",
            "    option \"B\"",
            "  button \"Go\"",
            "    StaticText \"Go\"",
            "  slider \"volume\"",
        ]
    );
    let find = |role: &str| nodes.iter().find(|n| n["role"]["value"] == role && n["ignored"] == false).unwrap().clone();
    let heading = find("heading");
    assert_eq!(prop(&heading, "level"), &json!({"type":"integer","value":1}));
    let textbox = find("textbox");
    assert_eq!(textbox["value"], json!({"type":"string","value":"v"}));
    assert_eq!(prop(&textbox, "required"), &json!({"type":"boolean","value":true}));
    assert_eq!(prop(&textbox, "editable")["value"], "plaintext");
    assert_eq!(prop(&textbox, "labelledby")["type"], "nodeList");
    assert_eq!(prop(&textbox, "labelledby")["relatedNodes"][0]["text"], "Name");
    assert_eq!(prop(&find("checkbox"), "checked"), &json!({"type":"tristate","value":"true"}));
    assert_eq!(prop(&find("link"), "url")["value"], "https://example.com/x");
    let combobox = find("combobox");
    assert_eq!(combobox["value"]["value"], "B");
    assert_eq!(prop(&combobox, "expanded")["value"], false);
    let button = find("button");
    assert_eq!(prop(&button, "disabled"), &json!({"type":"boolean","value":true}));
    assert!(prop(&button, "focusable").is_null(), "disabled is not focusable: {button}");
    assert_eq!(button["description"]["value"], "tip");
    let slider = find("slider");
    assert_eq!(slider["value"], json!({"type":"number","value":5}));
    assert_eq!(prop(&slider, "valuemax")["value"], 100);
    assert_eq!(prop(&find("listitem"), "level")["value"], 1);
    // aria-hidden content stays as ignored nodes; display:none content is absent.
    let hidden: Vec<&Value> =
        nodes.iter().filter(|n| n["ignored"] == true && n["ignoredReasons"][0]["name"] != "uninteresting").collect();
    let reasons: Vec<&str> = hidden.iter().map(|n| n["ignoredReasons"][0]["name"].as_str().unwrap()).collect();
    assert_eq!(reasons, ["ariaHiddenElement", "ariaHiddenSubtree", "ariaHiddenSubtree"], "{hidden:#?}");
    // Every child id resolves, and parent links agree.
    let map = by_id(&nodes);
    for n in &nodes {
        for c in n["childIds"].as_array().unwrap() {
            assert_eq!(map[c.as_str().unwrap()]["parentId"], n["nodeId"], "{n}");
        }
        assert!(n["backendDOMNodeId"].is_u64(), "{n}");
    }
}

#[test]
fn ax_node_ids_are_dom_backend_node_ids() {
    let mut h = Harness::new();
    load_html(&mut h, PAGE);
    let go = object(&mut h, "#go");
    let described = h.page("DOM.describeNode", json!({"objectId": go}));
    let backend = described["result"]["node"]["backendNodeId"].as_u64().unwrap();
    let tree = nodes(&h.page("Accessibility.getFullAXTree", json!({})));
    let button = tree.iter().find(|n| n["role"]["value"] == "button" && n["ignored"] == false).unwrap();
    assert_eq!(button["backendDOMNodeId"], backend);
    // And they resolve back through DOM.
    let r = h.page("DOM.resolveNode", json!({"backendNodeId": backend}));
    assert!(r["result"]["object"]["objectId"].is_string(), "{r}");
}

#[test]
fn full_ax_tree_depth_counts_unignored_levels_like_chrome() {
    let mut h = Harness::new();
    load_html(&mut h, "<title>Q</title><div id=root><h1>Head</h1><div><span id=sp>inner</span> <button>Go</button></div><p>Go</p></div>");
    let count =
        |h: &mut Harness, depth: i64| nodes(&h.page("Accessibility.getFullAXTree", json!({"depth": depth}))).len();
    // Chrome 1:1: root, html, body, #root at depth 0/1; + h1, div, p at 2;
    // + their text and span/button at 3; + the deepest text at 4.
    assert_eq!(count(&mut h, 0), 4);
    assert_eq!(count(&mut h, 1), 4);
    assert_eq!(count(&mut h, 2), 7);
    assert_eq!(count(&mut h, 3), 11);
    assert_eq!(count(&mut h, 4), 13);
    let r = h.page("Accessibility.getFullAXTree", json!({"frameId": "nope"}));
    assert_eq!(r["error"]["code"], -32602, "{r}");
    assert_eq!(r["error"]["message"], "Frame with the given frameId is not found.", "{r}");
}

#[test]
fn partial_ax_tree_returns_node_children_and_ancestors() {
    let mut h = Harness::new();
    load_html(&mut h, "<title>Q</title><div id=root><div><span id=sp>inner</span> <button id=b>Go</button></div><div id=none style=display:none>x</div></div>");
    let b = object(&mut h, "#b");
    let r = nodes(&h.page("Accessibility.getPartialAXTree", json!({"objectId": b})));
    let roles: Vec<&str> = r.iter().map(|n| n["role"]["value"].as_str().unwrap()).collect();
    // Chrome 1:1: the button, its text, then ancestors up to the root.
    assert_eq!(roles, ["button", "StaticText", "generic", "generic", "none", "none", "RootWebArea"], "{r:#?}");
    let b = object(&mut h, "#b");
    let only = nodes(&h.page("Accessibility.getPartialAXTree", json!({"objectId": b, "fetchRelatives": false})));
    assert_eq!(only.len(), 1);
    assert_eq!(only[0]["name"]["value"], "Go");
    let none = object(&mut h, "#none");
    let r = nodes(&h.page("Accessibility.getPartialAXTree", json!({"objectId": none, "fetchRelatives": false})));
    assert_eq!(r[0]["ignored"], true);
    assert_eq!(r[0]["ignoredReasons"][0]["name"], "notRendered");
    assert!(r[0]["parentId"].is_string(), "{r:#?}");
    let r = h.page("Accessibility.getPartialAXTree", json!({"backendNodeId": 999_999}));
    assert_eq!(r["error"]["code"], -32000, "{r}");
}

#[test]
fn query_ax_tree_filters_role_and_name_in_subtree() {
    let mut h = Harness::new();
    load_html(
        &mut h,
        "<div id=root><h1>Head</h1><button id=b>Go</button><button aria-hidden=true>Hid</button><p>Go</p></div><button>Outside</button>",
    );
    let root = object(&mut h, "#root");
    let r = nodes(&h.page("Accessibility.queryAXTree", json!({"objectId": root, "role": "button"})));
    // Chrome 1:1: the aria-hidden button is returned too, ignored, with its role.
    let got: Vec<(String, bool)> =
        r.iter().map(|n| (n["name"]["value"].as_str().unwrap().to_owned(), n["ignored"].as_bool().unwrap())).collect();
    assert_eq!(got, [("Go".to_owned(), false), (String::new(), true)], "{r:#?}");
    let root = object(&mut h, "#root");
    let r = nodes(&h.page("Accessibility.queryAXTree", json!({"objectId": root, "accessibleName": "Go"})));
    let roles: Vec<&str> = r.iter().map(|n| n["role"]["value"].as_str().unwrap()).collect();
    assert_eq!(roles, ["button", "StaticText", "StaticText"], "{r:#?}");
    let b = object(&mut h, "#b");
    let r =
        nodes(&h.page("Accessibility.queryAXTree", json!({"objectId": b, "role": "button", "accessibleName": "Go"})));
    assert_eq!(r.len(), 1, "the root node itself matches");
    let r = h.page("Accessibility.queryAXTree", json!({"role": "button"}));
    assert_eq!(r["error"]["code"], -32602, "{r}");
}

#[test]
fn ax_tree_reads_live_form_state() {
    let mut h = Harness::new();
    load_html(&mut h, "<input id=t aria-label=t><input type=checkbox id=c aria-label=c><button id=f>f</button>");
    eval_value(&mut h, "const $ = (id) => document.getElementById(id); $('t').value = 'typed'; $('c').checked = true; $('f').focus(); 1");
    let tree = nodes(&h.page("Accessibility.getFullAXTree", json!({})));
    let find = |role: &str| tree.iter().find(|n| n["role"]["value"] == role).unwrap().clone();
    assert_eq!(find("textbox")["value"]["value"], "typed");
    assert_eq!(prop(&find("checkbox"), "checked")["value"], "true");
    assert_eq!(prop(&find("button"), "focused")["value"], true);
    assert!(prop(&tree[0], "focused").is_null(), "the root is not focused when an element is");
}

fn accessibility_events(messages: &[Value]) -> Vec<&Value> {
    messages.iter().filter(|m| m["method"].as_str().is_some_and(|m| m.starts_with("Accessibility."))).collect()
}

#[test]
fn enable_and_disable_ack_without_events() {
    let mut h = Harness::new();
    load_html(&mut h, "<button>b</button>");
    for method in ["Accessibility.enable", "Accessibility.disable"] {
        let messages = h.page_with_events(method, json!({}));
        let reply = messages.iter().find(|m| m.get("id").is_some()).unwrap();
        assert_eq!(reply["result"], json!({}), "{reply}");
        assert!(accessibility_events(&messages).is_empty(), "{messages:#?}");
    }
    h.page("Accessibility.enable", json!({}));
    eval_value(&mut h, "document.body.append(document.createElement('button')); 1");
    let later = h.run_for(50);
    assert!(accessibility_events(&later).is_empty(), "{later:#?}");
}

#[test]
fn child_frame_tree_by_frame_id() {
    let mut h = Harness::new();
    load_html(&mut h, "<title>top</title><iframe name=kid srcdoc='<title>inner</title><button>In</button>'></iframe>");
    h.run_for(150);
    let tree = h.page("Page.getFrameTree", json!({}));
    let child = tree["result"]["frameTree"]["childFrames"][0]["frame"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("{tree}"))
        .to_owned();
    let top = nodes(&h.page("Accessibility.getFullAXTree", json!({})));
    let iframe = top.iter().find(|n| n["role"]["value"] == "Iframe").unwrap();
    assert_eq!(iframe["childIds"], json!([]), "child frame contents are not inlined: {iframe}");
    let inner = nodes(&h.page("Accessibility.getFullAXTree", json!({"frameId": child})));
    assert_eq!(inner[0]["frameId"], json!(child));
    assert_eq!(inner[0]["name"]["value"], "inner");
    assert!(inner.iter().any(|n| n["role"]["value"] == "button" && n["name"]["value"] == "In"), "{inner:#?}");
}

#[test]
fn stale_and_missing_selectors_fail_like_dom() {
    let mut h = Harness::new();
    load_html(&mut h, "<button id=b>b</button>");
    let r = h.page("Accessibility.getPartialAXTree", json!({}));
    assert_eq!(r["error"]["message"], "Specify exactly one nodeId, backendNodeId or objectId", "{r}");
    let b = object(&mut h, "#b");
    let backend =
        h.page("DOM.describeNode", json!({"objectId": b}))["result"]["node"]["backendNodeId"].as_u64().unwrap();
    load_html(&mut h, "<p>next</p>");
    let r = h.page("Accessibility.getPartialAXTree", json!({"backendNodeId": backend}));
    assert_eq!(r["error"]["message"], "Could not find node with given id", "{r}");
    let root = object(&mut h, "body");
    let r = nodes(&h.page("Accessibility.queryAXTree", json!({"objectId": root, "accessibleName": "missing"})));
    assert!(r.is_empty());
    let root = object(&mut h, "body");
    let r = nodes(&h.page("Accessibility.queryAXTree", json!({"objectId": root, "accessibleName": "  next "})));
    assert_eq!(r.len(), 1, "names compare whitespace-normalized: {r:#?}");
}

#[test]
fn large_pages_keep_stable_ids_past_the_backend_budget() {
    let mut h = Harness::new();
    load_html(&mut h, "<body></body>");
    eval_value(
        &mut h,
        "for (let i = 0; i < 5000; i++) { const d = document.createElement('div'); d.textContent = 'row ' + i; document.body.append(d); } 1",
    );
    let ids = |h: &mut Harness| -> Vec<(String, Value)> {
        nodes(&h.page("Accessibility.getFullAXTree", json!({})))
            .into_iter()
            .map(|n| (n["nodeId"].as_str().unwrap().to_owned(), n["backendDOMNodeId"].clone()))
            .collect()
    };
    let first = ids(&mut h);
    assert_eq!(first.len(), 3 + 2 * 5000, "root, html, body, a generic and a text per row");
    // The backend id table holds 4096 entries: later nodes come without one,
    // but the tree is complete and AX ids do not depend on it.
    assert!(first.iter().any(|(_, b)| b.is_null()));
    assert!(first[..100].iter().all(|(_, b)| b.is_u64()));
    assert_eq!(ids(&mut h), first, "a second call reports identical ids");
}
