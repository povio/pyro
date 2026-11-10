//! WHATWG URL conformance, driven by the WPT data files (`url/resources/urltestdata.json`,
//! `setters_tests.json`, `toascii.json`), without a WPT run.
//!
//! Each case goes through exactly the calls the `URL` binding, `<a>`/`<area>` and `Location`
//! make (`Url::options().base_url(..).parse`, `url::quirks` getters and setters), so these counts
//! are what those APIs see. Two data sets are checked:
//! - the subset committed under `tests/data/url/` (always), which must pass except for the cases
//!   listed in `tests/data/url/expected_failures.txt`;
//! - the full upstream files, when a WPT checkout is found (`WPT_DIR`, or a
//!   `lightningpanda-bench-work/wpt-upstream` next to an ancestor of this crate), with the same
//!   expectation. Missing files skip that part.
//!
//! The vendored subset also runs through the JavaScript bindings (`URL`, `<a>`), together with
//! `urltestdata-javascript-only.json`, whose lone surrogates JSON for Rust cannot hold.
//!
//! `cargo test -p lp-webapi --test integration url_conformance -- --nocapture` prints the counts.
use serde_json::Value;
use std::path::{Path, PathBuf};
use url::{quirks, Url};

const DATA: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/url");

#[derive(Default)]
struct Tally {
    pass: usize,
    fail: Vec<String>,
}
impl Tally {
    fn check(&mut self, name: String, problems: Vec<String>) {
        if problems.is_empty() {
            self.pass += 1;
        } else {
            self.fail.push(format!("{name} -- {}", problems.join("; ")));
        }
    }
    fn total(&self) -> usize {
        self.pass + self.fail.len()
    }
}

fn field(url: &Url, name: &str) -> String {
    match name {
        "href" => quirks::href(url).to_owned(),
        "origin" => quirks::origin(url),
        "protocol" => quirks::protocol(url).to_owned(),
        "username" => quirks::username(url).to_owned(),
        "password" => quirks::password(url).to_owned(),
        "host" => quirks::host(url).to_owned(),
        "hostname" => quirks::hostname(url).to_owned(),
        "port" => quirks::port(url).to_owned(),
        "pathname" => quirks::pathname(url).to_owned(),
        "search" => quirks::search(url).to_owned(),
        "hash" => quirks::hash(url).to_owned(),
        _ => unreachable!("{name}"),
    }
}

const FIELDS: [&str; 11] =
    ["href", "origin", "protocol", "username", "password", "host", "hostname", "port", "pathname", "search", "hash"];

/// `new URL(input, base)`, as the binding does it.
fn construct(input: &str, base: Option<&str>) -> Option<Url> {
    let base = match base {
        Some(base) => Some(Url::parse(base).ok()?),
        None => None,
    };
    Url::options().base_url(base.as_ref()).parse(input).ok()
}

fn compare(url: &Url, expected: &serde_json::Map<String, Value>) -> Vec<String> {
    let mut problems = vec![];
    for name in FIELDS {
        let Some(want) = expected.get(name).and_then(Value::as_str) else { continue };
        let got = field(url, name);
        if got != want {
            problems.push(format!("{name}: got {got:?}, want {want:?}"));
        }
    }
    problems
}

fn run_urltestdata(data: &Value) -> Tally {
    let mut tally = Tally::default();
    for case in data.as_array().unwrap() {
        let Some(case) = case.as_object() else { continue };
        let input = case["input"].as_str().unwrap();
        let base = case.get("base").and_then(Value::as_str);
        let mut name = format!("<{}>", input.escape_default());
        if let Some(base) = base {
            name += &format!(" against <{}>", base.escape_default());
        }
        let url = construct(input, base);
        let problems = match (url, case.get("failure").is_some()) {
            (None, true) => vec![],
            (Some(url), true) => vec![format!("parsed as {:?}, want failure", url.as_str())],
            (None, false) => vec!["failed to parse".into()],
            (Some(url), false) => compare(&url, case),
        };
        tally.check(name, problems);
    }
    tally
}

/// A setter as the binding calls it; a failed component parse leaves the URL unchanged.
fn set(url: &mut Url, property: &str, value: &str) {
    let _ = match property {
        "href" => quirks::set_href(url, value).map_err(|_| ()),
        "protocol" => quirks::set_protocol(url, value),
        "username" => quirks::set_username(url, value),
        "password" => quirks::set_password(url, value),
        "host" => quirks::set_host(url, value),
        "hostname" => quirks::set_hostname(url, value),
        "port" => quirks::set_port(url, value),
        "pathname" => {
            quirks::set_pathname(url, value);
            Ok(())
        }
        "search" => {
            quirks::set_search(url, value);
            Ok(())
        }
        "hash" => {
            quirks::set_hash(url, value);
            Ok(())
        }
        _ => unreachable!("{property}"),
    };
}

fn run_setters(data: &Value) -> Tally {
    let mut tally = Tally::default();
    for (property, cases) in data.as_object().unwrap() {
        if property == "comment" {
            continue;
        }
        for case in cases.as_array().unwrap() {
            let href = case["href"].as_str().unwrap();
            let value = case["new_value"].as_str().unwrap();
            let name = format!("<{}> set {property} to <{}>", href.escape_default(), value.escape_default());
            let problems = match Url::parse(href) {
                Err(_) => vec!["href failed to parse".into()],
                Ok(mut url) => {
                    set(&mut url, property, value);
                    compare(&url, case["expected"].as_object().unwrap())
                }
            };
            tally.check(name, problems);
        }
    }
    tally
}

fn run_toascii(data: &Value) -> Tally {
    let mut tally = Tally::default();
    for case in data.as_array().unwrap() {
        let Some(case) = case.as_object() else { continue };
        let input = case["input"].as_str().unwrap();
        let output = case["output"].as_str();
        let escaped = input.escape_default();
        let problems = match (Url::parse(&format!("https://{input}/x")), output) {
            (Err(_), None) => vec![],
            (Ok(url), None) => vec![format!("parsed as {:?}, want failure", url.as_str())],
            (Err(_), Some(_)) => vec!["failed to parse".into()],
            (Ok(url), Some(output)) => {
                let mut expected = serde_json::Map::new();
                for name in ["host", "hostname"] {
                    expected.insert(name.into(), output.into());
                }
                expected.insert("pathname".into(), "/x".into());
                expected.insert("href".into(), format!("https://{output}/x").into());
                compare(&url, &expected)
            }
        };
        tally.check(format!("toascii <{escaped}>"), problems);
        for property in ["host", "hostname"] {
            let mut url = Url::parse("https://x/x").unwrap();
            set(&mut url, property, input);
            let got = field(&url, property);
            let want = output.unwrap_or("x");
            let problems = if got == want { vec![] } else { vec![format!("{property}: got {got:?}, want {want:?}")] };
            tally.check(format!("toascii <{escaped}> via {property} setter"), problems);
        }
    }
    tally
}

fn load(dir: &Path, file: &str) -> Option<Value> {
    let text = std::fs::read_to_string(dir.join(file)).ok()?;
    Some(serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", dir.join(file).display())))
}

fn expected_failures() -> Vec<String> {
    std::fs::read_to_string(Path::new(DATA).join("expected_failures.txt"))
        .unwrap()
        .lines()
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(str::to_owned)
        .collect()
}

/// Runs the three files of `dir`; returns the unexpected failures.
fn run_dir(label: &str, dir: &Path) -> Option<Vec<String>> {
    let expected = expected_failures();
    let mut unexpected = vec![];
    let mut any = false;
    for (file, run) in [
        ("urltestdata.json", run_urltestdata as fn(&Value) -> Tally),
        ("setters_tests.json", run_setters),
        ("toascii.json", run_toascii),
    ] {
        let Some(data) = load(dir, file) else {
            println!("{label} {file}: not found, skipped");
            continue;
        };
        any = true;
        let tally = run(&data);
        println!("{label} {file}: {}/{} pass", tally.pass, tally.total());
        for failure in tally.fail {
            let known = expected.iter().any(|name| failure.split(" -- ").next() == Some(name.as_str()));
            println!("  {} {failure}", if known { "known" } else { "FAIL " });
            if !known {
                unexpected.push(failure);
            }
        }
    }
    any.then_some(unexpected)
}

fn upstream_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("WPT_DIR") {
        return Some(PathBuf::from(dir).join("url/resources"));
    }
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .map(|a| a.join("lightningpanda-bench-work/wpt-upstream/url/resources"))
        .find(|p| p.is_dir())
}

/// The same vendored data through the JavaScript bindings: `new URL`, `URL.canParse`, the `URL`
/// setters, and `<a>` (`HTMLHyperlinkElementUtils`) parsing and setters, as WPT's
/// `url-constructor`, `url-setters`, `url-setters-a-area`, `a-element` and `toascii` tests do.
const JS_RUNNER: &str = r#"(() => {
  const props = ["href", "origin", "protocol", "username", "password", "host", "hostname", "port", "pathname", "search", "hash"];
  const fail = [];
  let pass = 0;
  const check = (name, problems) => problems.length ? fail.push(name + " -- " + problems.join("; ")) : pass++;
  const cmp = (obj, exp) => props.filter(k => k in exp && obj[k] !== exp[k])
    .map(k => `${k}: got ${JSON.stringify(obj[k])}, want ${JSON.stringify(exp[k])}`);
  const anchor = href => { const a = document.createElement("a"); a.href = href; return a; };
  for (const t of DATA.tests) {
    if (typeof t === "string") continue;
    const base = t.base === null ? undefined : t.base;
    const name = `<${t.input}>` + (base === undefined ? "" : ` against <${base}>`);
    let url = null;
    try { url = new URL(t.input, base); } catch (e) { if (!(e instanceof TypeError)) throw e; }
    check("URL " + name, t.failure ? (url ? [`parsed as ${url.href}`] : []) : url ? cmp(url, t) : ["threw"]);
    check("URL.canParse " + name, URL.canParse(t.input, base) === !t.failure ? [] : ["wrong"]);
    // a-element.js: <a href> against a <base>, about:blank standing in for no base.
    if (t.relativeTo === "any-base" || /^(data|javascript):/.test(base ?? "")) continue;
    document.getElementById("base").href = base ?? "about:blank";
    const a = document.createElement("a");
    a.setAttribute("href", t.input);
    check("<a> " + name, t.failure
      ? (a.protocol === ":" && a.href === t.input ? [] : [`parsed as ${a.href}`])
      : cmp(a, t));
  }
  for (const [prop, cases] of Object.entries(DATA.setters)) {
    if (prop === "comment") continue;
    for (const t of cases) {
      const name = `<${t.href}>.${prop} = <${t.new_value}>`;
      const url = new URL(t.href);
      url[prop] = t.new_value;
      check("URL " + name, cmp(url, t.expected));
      const a = anchor(t.href);
      a[prop] = t.new_value;
      check("<a> " + name, cmp(a, t.expected));
    }
  }
  for (const t of DATA.toascii) {
    if (typeof t === "string") continue;
    const input = "https://" + t.input + "/x";
    let url = null;
    try { url = new URL(input); } catch (e) {}
    const want = t.output === null ? null : { host: t.output, hostname: t.output, pathname: "/x", href: "https://" + t.output + "/x" };
    check("URL toascii " + t.input, want ? (url ? cmp(url, want) : ["threw"]) : url ? ["parsed"] : []);
    check("<a> toascii " + t.input, cmp(anchor(input), want || { host: "", hostname: "", pathname: "", href: input }));
    for (const prop of ["host", "hostname"]) {
      for (const obj of [new URL("https://x/x"), anchor("https://x/x")]) {
        obj[prop] = t.input;
        check(`${obj instanceof URL ? "URL" : "<a>"}.${prop} toascii ${t.input}`, obj[prop] === (t.output ?? "x") ? [] : [obj[prop]]);
      }
    }
  }
  return JSON.stringify({ pass, fail });
})()"#;

#[test]
fn url_conformance_vendored_subset_through_js_bindings() {
    let data = |file| std::fs::read_to_string(Path::new(DATA).join(file)).unwrap();
    let source = format!(
        "const DATA = {{ tests: [...{}, ...{}], setters: {}, toascii: {} }};\n{JS_RUNNER}",
        data("urltestdata.json"),
        // Lone surrogates, which only a JavaScript string can hold.
        data("urltestdata-javascript-only.json"),
        data("setters_tests.json"),
        data("toascii.json")
    );
    let env = crate::dom_js::env("<head><base id=base></head><body></body>");
    let result = crate::dom_js::eval(&env.main, &source);
    let result: Value = serde_json::from_str(&result).unwrap_or_else(|_| panic!("{result}"));
    let fail: Vec<&str> = result["fail"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
    println!("JS bindings: {} pass, {} fail", result["pass"], fail.len());
    assert!(fail.is_empty(), "{} failures:\n{}", fail.len(), fail.join("\n"));
}

#[test]
fn url_conformance_vendored_subset() {
    let unexpected = run_dir("vendored", Path::new(DATA)).expect("vendored URL data is committed");
    assert!(unexpected.is_empty(), "{} unexpected failures:\n{}", unexpected.len(), unexpected.join("\n"));
}

#[test]
fn url_conformance_upstream_wpt_data() {
    let Some(dir) = upstream_dir() else {
        println!("no WPT checkout found (set WPT_DIR); skipped");
        return;
    };
    let Some(unexpected) = run_dir("upstream", &dir) else { return };
    assert!(unexpected.is_empty(), "{} unexpected failures:\n{}", unexpected.len(), unexpected.join("\n"));
}
