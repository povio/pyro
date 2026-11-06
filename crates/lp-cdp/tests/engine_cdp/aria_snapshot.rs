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
    let r = h.page("LP.query", json!({"locator": {"css": "body"}, "read": "aria"}));
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
    let r = h.page("LP.query", json!({"locator": {"css": css}, "read": "aria"}));
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
