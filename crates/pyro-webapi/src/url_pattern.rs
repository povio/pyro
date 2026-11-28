//! URL Pattern Standard. rust-urlpattern parses and canonicalizes patterns;
//! the owning JS engine compiles and executes their ECMAScript expressions.
use std::borrow::Cow;

use pyro_js::{Args, ClassDef, JsClass, JsError, JsResult, NoParent, Object, RootedVec, Scope, Value};
use urlpattern::{quirks, regexp::RegExp, RegexSyntax, UrlPatternOptions};

const COMPONENTS: [&str; 8] = ["protocol", "username", "password", "hostname", "port", "pathname", "search", "hash"];
const REGEXPS: &str = "lpURLPatternExpressions";
const REGEXP_HELPERS: &str = r#"({
    compile: (source, ignoreCase) => new RegExp(source, ignoreCase ? 'vi' : 'v'),
    match: (re, input) => RegExp.prototype.exec.call(re, input)
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
