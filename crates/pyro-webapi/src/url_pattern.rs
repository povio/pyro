//! URL Pattern Standard. rust-urlpattern parses and canonicalizes patterns;
//! the owning JS engine compiles and executes their ECMAScript expressions.
use std::borrow::Cow;

use pyro_js::{Args, ClassDef, JsClass, JsError, JsResult, NoParent, Object, RootedVec, Scope, Value};
use std::cmp::Ordering;
use std::collections::HashMap;

use urlpattern::parser::{parse_pattern_string, Options, Part, PartModifier, PartType};
use urlpattern::{quirks, regexp::RegExp, RegexSyntax, UrlPatternOptions};

const COMPONENTS: [&str; 8] = ["protocol", "username", "password", "hostname", "port", "pathname", "search", "hash"];
const REGEXPS: &str = "lpURLPatternExpressions";
const REGEXP_HELPERS: &str = r#"({
    compile: (source, ignoreCase) => new RegExp(source, ignoreCase ? 'vi' : 'v'),
    match: (re, input) => RegExp.prototype.exec.call(re, input),
    entries: (record) => Object.entries(record)
})"#;

fn helpers<'s>(s: &Scope<'s>) -> JsResult<Object<'s>> {
    Ok(s.agent_value("URLPattern regexp helpers", REGEXP_HELPERS)?.as_object().expect("regexp helpers"))
}

// During parsing the library only tests the protocol against the six fixed
// special-scheme strings. Page-provided URL inputs are matched by JSC.
struct Expression(String, String);
impl RegExp for Expression {
    fn syntax() -> RegexSyntax {
        RegexSyntax::EcmaScript
    }
    fn parse(source: &str, flags: &str, _: bool) -> Result<Self, ()> {
        Ok(Self(source.to_owned(), flags.replace('u', "v")))
    }
    fn matches<'a>(&self, scheme: &'a str) -> Option<Vec<Option<&'a str>>> {
        let hit = regress::Regex::with_flags(&self.0, self.1.as_str()).ok()?.find(scheme)?;
        Some(hit.captures.into_iter().map(|r| r.map(|r| &scheme[r])).collect())
    }
    fn pattern_string(&self) -> &str {
        &self.0
    }
}

type Input = quirks::StringOrInit<'static>;

fn input(value: Value<'_>) -> JsResult<Input> {
    if value.is_nullish() {
        return Ok(Input::Init(Default::default()));
    }
    let Some(object) = value.as_object() else { return Ok(Input::String(Cow::Owned(value.to_string()?))) };
    let member = |name| {
        let v = object.get_atom(name)?;
        if v.is_undefined() {
            Ok(None)
        } else {
            v.to_string().map(Some)
        }
    };
    // Web IDL converts dictionary members in lexicographic order.
    Ok(Input::Init(quirks::UrlPatternInit {
        base_url: member("baseURL")?,
        hash: member("hash")?,
        hostname: member("hostname")?,
        password: member("password")?,
        pathname: member("pathname")?,
        port: member("port")?,
        protocol: member("protocol")?,
        search: member("search")?,
        username: member("username")?,
    }))
}

fn init(input: Input, base: Option<&str>) -> Result<urlpattern::UrlPatternInit, urlpattern::Error> {
    if let Input::String(pattern) = input {
        let base = base.map(url::Url::parse).transpose().map_err(urlpattern::Error::Url)?;
        urlpattern::UrlPatternInit::parse_constructor_string::<Expression>(&pattern, base)
    } else {
        quirks::process_construct_pattern_input(input, base)
    }
}

fn type_error(error: urlpattern::Error) -> JsError {
    JsError::Type(error.to_string().into())
}

fn components(p: &quirks::UrlPattern) -> [&quirks::UrlPatternComponent; 8] {
    [&p.protocol, &p.username, &p.password, &p.hostname, &p.port, &p.pathname, &p.search, &p.hash]
}

// URLPattern.compareComponent / generate are the tentative extensions the WPT
// `urlpattern-compare` / `urlpattern-generate` tests cover (explainer:
// explainers-by-googlers/urlpattern-generate). Both work on a component's part
// list, recovered by parsing the component's normalized pattern string again
// with the options the pattern was compiled with (the crate keeps only the
// string and the regexp, not the parts).

// WebIDL enum URLPatternComponent: an unknown value is a TypeError.
fn component_index(name: &str) -> JsResult<usize> {
    COMPONENTS
        .iter()
        .position(|c| *c == name)
        .ok_or_else(|| JsError::Type(format!("'{name}' is not a valid URLPattern component").into()))
}

fn protocol_is_special(p: &quirks::UrlPattern) -> bool {
    let Ok(re) = Expression::parse(&p.protocol.regexp_string, "u", false) else { return false };
    ["ftp", "file", "http", "https", "ws", "wss"].iter().any(|scheme| re.matches(scheme).is_some())
}

// The options the pattern compiler used for the component (a pathname uses the
// "/"-delimited ones only while it is not an opaque path).
fn component_options(p: &quirks::UrlPattern, index: usize) -> Options {
    let mut options = match index {
        3 => Options::hostname(),
        5 if p.protocol.pattern_string.is_empty()
            || protocol_is_special(p)
            || p.pathname.pattern_string.starts_with('/') =>
        {
            Options::pathname()
        }
        _ => Options::default(),
    };
    options.regex_syntax = RegexSyntax::EcmaScript;
    options
}

fn component_parts(p: &quirks::UrlPattern, index: usize) -> JsResult<Vec<Part>> {
    // The pattern string is already canonical: nothing to encode again.
    parse_pattern_string(&components(p)[index].pattern_string, &component_options(p, index), |v| Ok(v.to_owned()))
        .map_err(type_error)
}

fn compare_strings(a: &str, b: &str) -> Ordering {
    // Code unit order, as for JavaScript strings.
    a.encode_utf16().cmp(b.encode_utf16())
}

// More specific sorts later: fixed text, then regexp, segment wildcard, full
// wildcard; an unmodified part, then `+`, `?`, `*`.
fn compare_parts(a: &Part, b: &Part) -> Ordering {
    let kind = |p: &Part| match p.kind {
        PartType::FixedText => 3,
        PartType::Regexp => 2,
        PartType::SegmentWildcard => 1,
        PartType::FullWildcard => 0,
    };
    let modifier = |p: &Part| match p.modifier {
        PartModifier::None => 3,
        PartModifier::OneOrMore => 2,
        PartModifier::Optional => 1,
        PartModifier::ZeroOrMore => 0,
    };
    // Group names never matter. Fixed text and regexps differ by their text.
    fn text(p: &Part) -> &str {
        if matches!(p.kind, PartType::FixedText | PartType::Regexp) {
            &p.value
        } else {
            ""
        }
    }
    kind(a)
        .cmp(&kind(b))
        .then_with(|| modifier(a).cmp(&modifier(b)))
        .then_with(|| compare_strings(text(a), text(b)))
        .then_with(|| compare_strings(&a.prefix, &b.prefix))
        .then_with(|| compare_strings(&a.suffix, &b.suffix))
}

fn compare_part_lists(a: &[Part], b: &[Part]) -> i32 {
    let ordering = a
        .iter()
        .zip(b)
        .map(|(x, y)| compare_parts(x, y))
        .find(|o| o.is_ne())
        .unwrap_or_else(|| a.len().cmp(&b.len()));
    ordering as i32
}

// record<USVString, USVString>: own enumerable string keys in order, every
// value converted with ToString.
fn record(s: &Scope<'_>, value: Value<'_>) -> JsResult<HashMap<String, String>> {
    if value.as_object().is_none() {
        return Err(JsError::Type("Expected a record of group names to values".into()));
    }
    let entries = helpers(s)?.get_atom("entries")?.as_object().expect("entries function");
    let list = entries.call(None, &[value])?.as_object().expect("entries array");
    let mut out = HashMap::new();
    for i in 0..list.get_atom("length")?.to_number()? as u32 {
        let pair = list.get_index(i)?.as_object().expect("entry pair");
        out.insert(pair.get_index(0)?.to_string()?, pair.get_index(1)?.to_string()?);
    }
    Ok(out)
}

// The component's encoding callback (canonicalize a protocol, username, ...)
// applied to one group value. A pathname is encoded as an opaque path when the
// pattern's protocol is not a special scheme, as for a URL with that scheme.
fn encode_group(p: &quirks::UrlPattern, index: usize, value: &str) -> Option<String> {
    if value.is_empty() {
        return Some(String::new());
    }
    let dummy = || url::Url::parse("http://dummy.test").expect("dummy URL");
    Some(match index {
        0 => url::Url::parse(&format!("{value}://dummy.test")).ok()?.scheme().to_owned(),
        1 => {
            let mut u = dummy();
            u.set_username(value).ok()?;
            u.username().to_owned()
        }
        2 => {
            let mut u = dummy();
            u.set_password(Some(value)).ok()?;
            u.password()?.to_owned()
        }
        // The compiler's rule for an IPv6 hostname pattern (`[`, `{[` or `\\[` first).
        3 if ["[", "{[", "\\["].iter().any(|lead| p.hostname.pattern_string.starts_with(*lead)) => {
            let valid = value.chars().all(|c| c.is_ascii_hexdigit() || matches!(c, '[' | ']' | ':'));
            if !valid {
                return None;
            }
            value.to_ascii_lowercase()
        }
        3 => {
            let mut u = dummy();
            url::quirks::set_hostname(&mut u, value).ok()?;
            url::quirks::hostname(&u).to_owned()
        }
        4 => {
            let mut u = url::Url::parse("dummy://dummy.test").expect("dummy URL");
            url::quirks::set_port(&mut u, value).ok()?;
            url::quirks::port(&u).to_owned()
        }
        5 if !p.protocol.pattern_string.is_empty() && !protocol_is_special(p) => {
            // URL "opaque path" state: the C0 control percent-encode set.
            value.bytes().fold(String::new(), |mut out, b| {
                if b < 0x20 || b > 0x7e {
                    out.push_str(&format!("%{b:02X}"));
                } else {
                    out.push(b as char);
                }
                out
            })
        }
        5 => {
            let leading_slash = value.starts_with('/');
            let mut u = dummy();
            u.set_path(&if leading_slash { value.to_owned() } else { format!("/-{value}") });
            let path = url::quirks::pathname(&u);
            // A "/-" prefix that survived parsing is the one added above.
            match path.strip_prefix("/-") {
                Some(rest) if !leading_slash => rest.to_owned(),
                _ => path.to_owned(),
            }
        }
        6 => {
            let mut u = dummy();
            u.set_query(Some(value));
            u.query().unwrap_or("").to_owned()
        }
        _ => {
            let mut u = dummy();
            u.set_fragment(Some(value));
            u.fragment().unwrap_or("").to_owned()
        }
    })
}

// Builds the component string from fixed text and one value per named segment
// group. Features that need a regexp to be inverted (custom regexp groups,
// wildcards, modifiers) are a TypeError, as is a group without a value or a
// value that its segment cannot match once encoded.
fn generate(p: &quirks::UrlPattern, index: usize, groups: &HashMap<String, String>) -> JsResult<String> {
    let options = component_options(p, index);
    let unsupported = || JsError::Type("URLPattern.generate does not support this pattern".into());
    let mut out = String::new();
    for part in component_parts(p, index)? {
        if part.modifier != PartModifier::None {
            return Err(unsupported());
        }
        match part.kind {
            PartType::FixedText => out.push_str(&part.value),
            PartType::SegmentWildcard => {
                let value = groups
                    .get(&part.name)
                    .ok_or_else(|| JsError::Type(format!("Missing value for group '{}'", part.name).into()))?;
                let encoded = encode_group(p, index, value)
                    .ok_or_else(|| JsError::Type(format!("Invalid value for group '{}'", part.name).into()))?;
                // The segment wildcard regexp: one or more non-delimiter characters.
                if encoded.is_empty() || options.delimiter_code_point.is_some_and(|d| encoded.contains(d)) {
                    return Err(JsError::Type(format!("Value for group '{}' does not match", part.name).into()));
                }
                out.push_str(&part.prefix);
                out.push_str(&encoded);
                out.push_str(&part.suffix);
            }
            PartType::Regexp | PartType::FullWildcard => return Err(unsupported()),
        }
    }
    Ok(out)
}

// Define own data properties even if an author installed a setter on the
// prototype. Web IDL dictionary and record conversions return ordinary objects.
fn dictionary<'s>(s: &Scope<'s>, fill: impl FnOnce(Object<'s>) -> JsResult<()>) -> JsResult<Value<'s>> {
    let object = s.object();
    let proto = object.prototype();
    object.set_prototype(s.null());
    fill(object)?;
    object.set_prototype(proto);
    Ok(object.value())
}

fn original_input<'s>(s: &Scope<'s>, value: &Input) -> JsResult<Value<'s>> {
    match value {
        Input::String(v) => Ok(s.string(v)),
        Input::Init(v) => dictionary(s, |o| {
            for (name, value) in [
                ("baseURL", &v.base_url),
                ("hash", &v.hash),
                ("hostname", &v.hostname),
                ("password", &v.password),
                ("pathname", &v.pathname),
                ("port", &v.port),
                ("protocol", &v.protocol),
                ("search", &v.search),
                ("username", &v.username),
            ] {
                if let Some(value) = value {
                    o.set_atom(name, s.string(value))?;
                }
            }
            Ok(())
        }),
    }
}

/// One entry of an `href_matches` predicate (speculation rules): a pattern string, or a
/// `URLPatternInit` dictionary (JSON object).
#[derive(Clone, Debug)]
pub enum PatternInput {
    String(String),
    Init(serde_json::Map<String, serde_json::Value>),
}

/// Speculation rules' `href_matches`: for each of `urls`, whether it matches any of `patterns`
/// (HTML "build a URL pattern from an Infra value": a string is a constructor string, a
/// dictionary without `baseURL` takes `base`; both resolve against it). `None` when a pattern is
/// invalid (the predicate, and with it the rule, is then unusable). Runs the same engine as
/// `URLPattern.prototype.test` without the page's globals: the compiled expressions come from the
/// agent's own helpers.
pub fn href_matches<'s>(s: &Scope<'s>, patterns: &[PatternInput], base: &str, urls: &[String]) -> Option<Vec<bool>> {
    let helpers = helpers(s).ok()?;
    let compile = helpers.get_atom("compile").ok()?.as_object()?;
    let execute = helpers.get_atom("match").ok()?.as_object()?;
    let parsed: Vec<Option<quirks::MatchInput>> = urls
        .iter()
        .map(|u| {
            let (normalized, _) = quirks::process_match_input(Input::String(Cow::Owned(u.clone())), None).ok()??;
            quirks::parse_match_input(normalized)
        })
        .collect();
    let mut out = vec![false; urls.len()];
    for pattern in patterns {
        let built = match pattern {
            PatternInput::String(text) => init(Input::String(Cow::Owned(text.clone())), Some(base)).ok()?,
            PatternInput::Init(map) => {
                let member = |name: &str| match map.get(name) {
                    None => Some(None),
                    Some(serde_json::Value::String(v)) => Some(Some(v.clone())),
                    Some(_) => None,
                };
                let dictionary = quirks::UrlPatternInit {
                    base_url: member("baseURL")?.or_else(|| Some(base.to_owned())),
                    hash: member("hash")?,
                    hostname: member("hostname")?,
                    password: member("password")?,
                    pathname: member("pathname")?,
                    port: member("port")?,
                    protocol: member("protocol")?,
                    search: member("search")?,
                    username: member("username")?,
                };
                quirks::process_construct_pattern_input(Input::Init(dictionary), None).ok()?
            }
        };
        let pattern = quirks::parse_pattern::<Expression>(
            built,
            UrlPatternOptions { regex_syntax: RegexSyntax::EcmaScript, ignore_case: false },
        )
        .ok()?;
        let mut expressions = RootedVec::new();
        for component in components(&pattern) {
            let compiled = compile.call(None, &[s.string(&component.regexp_string), s.boolean(false)]).ok()?;
            expressions.push(s, compiled);
        }
        for (i, input) in parsed.iter().enumerate() {
            let Some(parts) = input else { continue };
            if out[i] {
                continue;
            }
            let values = [
                &parts.protocol,
                &parts.username,
                &parts.password,
                &parts.hostname,
                &parts.port,
                &parts.pathname,
                &parts.search,
                &parts.hash,
            ];
            out[i] = values.iter().enumerate().all(|(k, value)| {
                let expression = expressions.get(s, k).expect("a compiled component");
                execute.call(None, &[expression, s.string(value)]).ok().is_some_and(|hit| hit.as_object().is_some())
            });
        }
    }
    Some(out)
}

pub(crate) struct URLPattern;
impl JsClass for URLPattern {
    const NAME: &'static str = "URLPattern";
    type Data = quirks::UrlPattern;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.constructor(0, |s, nt, a| {
            let value = input(a.get(0))?;
            let second = a.get(1);
            let has_base = a.len() >= 3 || (!second.is_nullish() && second.as_object().is_none());
            let base = if has_base { Some(second.to_string()?) } else { None };
            let options = a.get(if has_base { 2 } else { 1 });
            let ignore = if options.is_nullish() {
                false
            } else {
                options
                    .as_object()
                    .ok_or_else(|| JsError::Type("Expected URLPatternOptions".into()))?
                    .get_atom("ignoreCase")?
                    .to_bool()
            };
            let init = init(value, base.as_deref()).map_err(type_error)?;
            let pattern = quirks::parse_pattern::<Expression>(
                init,
                UrlPatternOptions { regex_syntax: RegexSyntax::EcmaScript, ignore_case: ignore },
            )
            .map_err(type_error)?;
            let compile = helpers(s)?.get_atom("compile")?.as_object().expect("compile function");
            let mut expressions = RootedVec::new();
            for (index, component) in components(&pattern).into_iter().enumerate() {
                expressions.push(
                    s,
                    compile
                        .call(None, &[s.string(&component.regexp_string), s.boolean(ignore && index >= 5)])
                        .map_err(|_| JsError::Type("Invalid URLPattern regular expression".into()))?,
                );
            }
            let wrapper = s.wrap_new::<Self>(pattern, nt, None)?;
            s.set_hidden(wrapper, REGEXPS, s.array(&expressions.iter(s).collect::<Vec<_>>())?.value())?;
            Ok(wrapper)
        });
        macro_rules! getter {
            ($field:ident) => {
                def.getter(stringify!($field), |s, p| Ok(s.string(&p.$field.pattern_string)));
            };
        }
        getter!(protocol);
        getter!(username);
        getter!(password);
        getter!(hostname);
        getter!(port);
        getter!(pathname);
        getter!(search);
        getter!(hash);
        def.getter("hasRegExpGroups", |s, p| Ok(s.boolean(p.has_regexp_groups)));
        def.method_this("test", 0, |s, p, this, a| match_url(s, p, this, a, false));
        def.method_this("exec", 0, |s, p, this, a| match_url(s, p, this, a, true));
        def.method_this("generate", 2, |s, p, _this, a| {
            a.require(2, "URLPattern.generate")?;
            let index = component_index(&a.get(0).to_string()?)?;
            let groups = record(s, a.get(1))?;
            generate(p, index, &groups).map(|v| s.string(&v))
        });
        def.static_method("compareComponent", 3, |s, a| {
            a.require(3, "URLPattern.compareComponent")?;
            let index = component_index(&a.get(0).to_string()?)?;
            let pattern = |i| {
                s.unwrap_arg::<Self>(&a, i)
                    .ok_or_else(|| JsError::Type("Argument is not a URLPattern".into()))
            };
            let (left, right) = (pattern(1)?, pattern(2)?);
            let left = component_parts(left, index)?;
            let right = component_parts(right, index)?;
            Ok(s.number(compare_part_lists(&left, &right) as f64))
        });
    }
}

fn match_url<'s>(
    s: &Scope<'s>,
    p: &quirks::UrlPattern,
    this: Object<'s>,
    args: Args<'s>,
    result: bool,
) -> JsResult<Value<'s>> {
    let value = input(args.get(0))?;
    let base = args.get(1);
    let base = if base.is_undefined() { None } else { Some(base.to_string()?) };
    let miss = if result { s.null() } else { s.boolean(false) };
    let Some((normalized, original)) = quirks::process_match_input(value, base.as_deref()).map_err(type_error)? else {
        return Ok(miss);
    };
    let Some(parts) = quirks::parse_match_input(normalized) else { return Ok(miss) };
    let expressions = s.get_hidden(this, REGEXPS)?.and_then(|v| v.as_object()).expect("URLPattern expressions");
    let execute = helpers(s)?.get_atom("match")?.as_object().expect("match function");
    let values = [
        parts.protocol,
        parts.username,
        parts.password,
        parts.hostname,
        parts.port,
        parts.pathname,
        parts.search,
        parts.hash,
    ];
    let mut matches = RootedVec::new();
    for (i, v) in values.iter().enumerate() {
        let hit = execute.call(None, &[expressions.get_index(i as u32)?, s.string(v)])?;
        let Some(hit) = hit.as_object() else { return Ok(miss) };
        if result {
            matches.push(s, hit.value());
        }
    }
    if !result {
        return Ok(s.boolean(true));
    }
    let original_value = original_input(s, &original.0)?;
    let inputs = match original.1 {
        Some(base) => s.array(&[original_value, s.string(&base)])?,
        None => s.array(&[original_value])?,
    };
    let parts = components(p);
    dictionary(s, |out| {
        // Result dictionary properties are defined in Web IDL member order.
        for key in ["hash", "hostname", "inputs", "password", "pathname", "port", "protocol", "search", "username"] {
            if key == "inputs" {
                out.set_atom("inputs", inputs.value())?;
                continue;
            }
            let i = COMPONENTS.iter().position(|name| *name == key).expect("known component");
            let hit = matches.get(s, i).and_then(|v| v.as_object()).expect("matched component");
            let groups = dictionary(s, |o| {
                for (j, name) in parts[i].group_name_list.iter().enumerate() {
                    o.set(name, hit.get_index(j as u32 + 1)?)?;
                }
                Ok(())
            })?;
            out.set_atom(
                key,
                dictionary(s, |o| {
                    o.set_atom("groups", groups)?;
                    o.set_atom("input", s.string(&values[i]))
                })?,
            )?;
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pathname(pattern: &str) -> quirks::UrlPattern {
        let init = quirks::UrlPatternInit { pathname: Some(pattern.to_owned()), ..Default::default() };
        let init = quirks::process_construct_pattern_input(Input::Init(init), None).unwrap();
        let options = UrlPatternOptions { regex_syntax: RegexSyntax::EcmaScript, ignore_case: false };
        quirks::parse_pattern::<Expression>(init, options).unwrap()
    }

    fn compare(left: &str, right: &str) -> i32 {
        let (l, r) = (pathname(left), pathname(right));
        compare_part_lists(&component_parts(&l, 5).unwrap(), &component_parts(&r, 5).unwrap())
    }

    #[test]
    fn compares_specificity_of_pathnames() {
        assert_eq!(compare("/foo/bar", "/foo/:bar"), 1);
        assert_eq!(compare("/foo/:bar", "/foo/*"), 1);
        assert_eq!(compare("/foo/{bar}", "/foo/{bar}+"), 1);
        assert_eq!(compare("/foo/{bar}+", "/foo/{bar}?"), 1);
        assert_eq!(compare("/foo/{bar}?", "/foo/{bar}*"), 1);
        assert_eq!(compare("/foo/(123)", "/foo/(12)"), 1);
        assert_eq!(compare("/foo/:b", "/foo/:a"), 0);
        assert_eq!(compare("foo/:bar?/baz", "foo{/:bar}?/baz"), 0);
        assert_eq!(compare("foo/:bar?/baz", "foo{/:bar/}?baz"), -1);
        assert_eq!(compare("*/foo", "*"), 1);
    }

    #[test]
    fn generates_from_named_groups() {
        let groups = |pairs: &[(&str, &str)]| -> HashMap<String, String> {
            pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
        };
        let p = pathname("/foo/:bar");
        assert_eq!(generate(&p, 5, &groups(&[("bar", "a b")])).ok().as_deref(), Some("/foo/a%20b"));
        assert!(generate(&p, 5, &groups(&[])).is_err());
        assert!(generate(&p, 5, &groups(&[("bar", "a/b")])).is_err());
        assert!(generate(&pathname("/foo/*"), 5, &groups(&[])).is_err());
        assert!(generate(&pathname("/foo/:bar?"), 5, &groups(&[("bar", "x")])).is_err());
    }
}
