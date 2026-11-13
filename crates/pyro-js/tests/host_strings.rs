//! Strings between Rust and JavaScript: `Scope::string`, `Scope::string_utf16`,
//! `Value::to_string`, `JsString`, JSON and property names, on the patched
//! JavaScriptCore's direct entry points (`webkit/patches/0016`) and on the
//! system framework's `JSStringRef` path alike.

use pyro_js::*;

fn engine() -> Engine {
    Engine::new().expect("engine")
}

/// Host strings that exercise each conversion: empty, single characters (the
/// VM's own strings up to U+00FF), NUL, ASCII, Latin-1, beyond Latin-1, a
/// surrogate pair, the stack-buffer boundaries and long text.
fn samples() -> Vec<String> {
    let mut v: Vec<String> = [
        "",
        "a",
        "\0",
        "\u{7f}",
        "\u{80}",
        "é",
        "\u{ff}",
        "\u{100}",
        "ž",
        "€",
        "🐸",
        "a\0b",
        "id",
        "item row-3",
        "café",
        "über café 12",
        "žabe 🐸",
        "\u{ff}\u{100}",
        "漢字テキスト",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    // Around the transcoding stack buffers: 256 Latin-1 bytes, 128 UTF-16 units.
    for n in [255, 256, 257] {
        v.push("é".repeat(n));
        v.push("x".repeat(n));
    }
    for n in [127, 128, 129] {
        v.push("ž".repeat(n));
        v.push(format!("{}🐸", "ž".repeat(n - 2)));
    }
    v.push("x".repeat(1 << 20));
    v.push("café ".repeat(1 << 18));
    v.push("漢字🐸".repeat(1 << 17));
    v
}

#[test]
fn host_strings_reach_javascript_unchanged() {
    let e = engine();
    let r = e.create_realm(RealmOptions::default()).unwrap();
    let s = r.scope().unwrap();
    let same = s
        .eval(
            "(x, json) => { const u = JSON.parse(json); if (x.length !== u.length) return false; \
             for (let i = 0; i < u.length; i++) if (x.charCodeAt(i) !== u[i]) return false; return true; }",
            None,
            1,
        )
        .unwrap()
        .as_object()
        .unwrap();
    for text in samples() {
        let units: Vec<u16> = text.encode_utf16().collect();
        let json = serde_json_units(&units);
        let v = s.string(&text);
        assert!(v.is_string());
        let ok = same.call(None, &[v, s.string(&json)]).unwrap();
        assert!(ok.to_bool(), "{:?}", short(&text));
        assert_eq!(v.to_string().unwrap(), text, "to_string {:?}", short(&text));
        assert_eq!(v.as_string().as_deref(), Some(text.as_str()));
        let w = s.string_utf16(&units);
        assert!(w.strict_equals(v), "string_utf16 {:?}", short(&text));
        let j = JsString::new(&text);
        assert!(j.eq_str(&text), "JsString {:?}", short(&text));
        assert_eq!(j.to_string(), text);
        assert_eq!(j.len_utf16(), units.len());
        assert_eq!(j.utf16(), &units[..]);
        assert!(s.js_string(&j).strict_equals(v));
    }
}

#[test]
fn unpaired_surrogates_survive_utf16_and_become_replacement_in_rust() {
    let e = engine();
    let r = e.create_realm(RealmOptions::default()).unwrap();
    let s = r.scope().unwrap();
    let units = [0xD800u16, u16::from(b'a'), 0xDC00, 0xD83D, 0xDC38];
    let v = s.string_utf16(&units);
    let check =
        s.eval("(x) => [x.length, x.charCodeAt(0), x.charCodeAt(2), x.codePointAt(3)].join()", None, 1).unwrap();
    let out = check.as_object().unwrap().call(None, &[v]).unwrap().to_string().unwrap();
    assert_eq!(out, "5,55296,56320,128056");
    assert_eq!(v.to_string().unwrap(), "\u{FFFD}a\u{FFFD}🐸");
    if let Some(StringUnits::Utf16(u)) = v.string_units() {
        assert_eq!(u, &units);
    }
    // A lone surrogate made in JavaScript reads the same way.
    let lone = s.eval("'x\\uDBFF'", None, 1).unwrap();
    assert_eq!(lone.to_string().unwrap(), "x\u{FFFD}");
    assert!(s.string_utf16(&[]).strict_equals(s.string("")));
}

#[test]
fn values_convert_outside_callbacks_including_ropes() {
    let e = engine();
    let r = e.create_realm(RealmOptions::default()).unwrap();
    let s = r.scope().unwrap();
    // Concatenations are ropes until something reads their characters; here
    // the first read happens at the top level, where the API lock is not held.
    let rope = s.eval("var a = 'ab'.repeat(300); a + 'é' + a", None, 1).unwrap();
    let want = format!("{0}é{0}", "ab".repeat(300));
    assert_eq!(rope.to_string().unwrap(), want);
    let rope16 = s.eval("var b = 'ž'.repeat(300); b + '🐸' + b", None, 1).unwrap();
    assert_eq!(rope16.as_string().unwrap(), format!("{0}🐸{0}", "ž".repeat(300)));
    // Non-strings still go through ToString.
    assert_eq!(s.number(1.5).to_string().unwrap(), "1.5");
    assert_eq!(s.null().to_string().unwrap(), "null");
    assert_eq!(s.number(2.0).as_string(), None);
    let obj = s.eval("({ toString() { return 'žabe'; } })", None, 1).unwrap();
    assert_eq!(obj.to_string().unwrap(), "žabe");
    assert_eq!(obj.as_string(), None);
    let thrower = s.eval("({ toString() { throw new Error('no'); } })", None, 1).unwrap();
    assert!(thrower.to_string().is_err());
}

#[test]
fn json_and_property_names_read_stored_characters() {
    let e = engine();
    let r = e.create_realm(RealmOptions::default()).unwrap();
    let s = r.scope().unwrap();
    let v = s.eval("({ id: 'i1', 'café': 'über', 'žabe': '🐸', n: 1 })", None, 1).unwrap();
    assert_eq!(v.to_json(0).unwrap().unwrap(), r#"{"id":"i1","café":"über","žabe":"🐸","n":1}"#);
    let o = v.as_object().unwrap();
    assert_eq!(o.keys(), ["id", "café", "žabe", "n"]);
    assert_eq!(o.get("café").unwrap().to_string().unwrap(), "über");
    assert_eq!(o.get("žabe").unwrap().to_string().unwrap(), "🐸");
    o.set("ñ", s.string("ü")).unwrap();
    assert!(o.has("ñ"));
    assert_eq!(
        s.eval("x => x['ñ']", None, 1).unwrap().as_object().unwrap().call(None, &[v]).unwrap().to_string().unwrap(),
        "ü"
    );
    let big = s.eval("JSON.stringify({ s: 'x'.repeat(100000), t: 'é' })", None, 1).unwrap().to_string().unwrap();
    assert_eq!(big.len(), 100000 + 17);
    assert!(big.ends_with(r#""t":"é"}"#));
}

fn serde_json_units(units: &[u16]) -> String {
    let mut out = String::with_capacity(units.len() * 4 + 2);
    out.push('[');
    for (i, u) in units.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&u.to_string());
    }
    out.push(']');
    out
}

fn short(s: &str) -> String {
    s.chars().take(12).collect::<String>() + if s.chars().count() > 12 { "…" } else { "" }
}
