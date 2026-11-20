//! Server-Timing response-header metrics (RFC 9110 token/quoted-string).
use pyro_js::{ClassDef, JsClass, JsResult, NoParent, Scope, Value};
use pyro_net::http::HeaderMap;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Metric {
    pub name: String,
    pub duration: f64,
    pub description: String,
}

pub struct PerformanceServerTiming;

impl JsClass for PerformanceServerTiming {
    const NAME: &'static str = "PerformanceServerTiming";
    type Data = Metric;
    type Parent = NoParent;

    fn define(def: &mut ClassDef<Self>) {
        def.getter("name", |s, m| Ok(s.string(&m.name)));
        def.getter("duration", |s, m| Ok(s.number(m.duration)));
        def.getter("description", |s, m| Ok(s.string(&m.description)));
        def.method("toJSON", 0, |s, m, _| m.json(s));
    }
}

impl Metric {
    pub(crate) fn json<'s>(&self, s: &Scope<'s>) -> JsResult<Value<'s>> {
        let object = s.object();
        object.set("name", s.string(&self.name))?;
        object.set("duration", s.number(self.duration))?;
        object.set("description", s.string(&self.description))?;
        Ok(object.value())
    }
}

fn token_byte(c: u8) -> bool {
    c.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&c)
}

fn ows(input: &mut &[u8]) {
    *input = input.trim_ascii_start();
}

fn token<'a>(input: &mut &'a [u8]) -> &'a str {
    ows(input);
    let end = input.iter().position(|&c| !token_byte(c)).unwrap_or(input.len());
    let (value, rest) = input.split_at(end);
    *input = rest;
    std::str::from_utf8(value).expect("ASCII token")
}

fn parameter_value(input: &mut &[u8]) -> String {
    ows(input);
    let Some(mut rest) = input.strip_prefix(b"\"") else { return token(input).to_owned() };
    let mut value = String::new();
    while let Some((&byte, tail)) = rest.split_first() {
        rest = tail;
        if byte == b'"' {
            *input = rest;
            return value;
        }
        let byte = if byte == b'\\' {
            let Some((&escaped, tail)) = rest.split_first() else { break };
            rest = tail;
            escaped
        } else {
            byte
        };
        value.push(char::from(byte));
    }
    *input = &[];
    String::new()
}

/// HTML's rules for parsing floating-point number values consume the
/// numeric prefix. An exponent without digits does not extend that prefix.
fn parse_duration(value: &str) -> f64 {
    let value = value.trim_ascii_start();
    let bytes = value.as_bytes();
    let digits = |start: usize| bytes[start..].iter().take_while(|n| n.is_ascii_digit()).count();
    let sign = usize::from(matches!(bytes.first(), Some(b'+' | b'-')));
    let mut end = sign + digits(sign);
    if bytes.get(end) == Some(&b'.') {
        end += 1;
        end += digits(end);
    }
    if matches!(bytes.get(end), Some(b'E' | b'e')) {
        let exponent = end + 1 + usize::from(matches!(bytes.get(end + 1), Some(b'+' | b'-')));
        let length = digits(exponent);
        if length != 0 {
            end = exponent + length;
        }
    }
    value[..end].parse::<f64>().ok().filter(|n| n.is_finite()).unwrap_or(0.0)
}

fn parse_field(mut input: &[u8], metrics: &mut Vec<Metric>) {
    while !input.is_empty() {
        let name = token(&mut input);
        let mut duration = None;
        let mut description = None;
        // Ignore trailing garbage after a token or a quoted value up to
        // the next separator. A quoted semicolon or comma was consumed above.
        loop {
            let Some(separator) = input.iter().position(|c| matches!(c, b';' | b',')) else {
                input = &[];
                break;
            };
            let (delimiter, rest) = input[separator..].split_first().unwrap();
            input = rest;
            if *delimiter == b',' {
                break;
            }
            let parameter = token(&mut input);
            ows(&mut input);
            let value = if let Some(rest) = input.strip_prefix(b"=") {
                input = rest;
                parameter_value(&mut input)
            } else {
                String::new()
            };
            if parameter.eq_ignore_ascii_case("dur") && duration.is_none() {
                duration = Some(parse_duration(&value));
            } else if parameter.eq_ignore_ascii_case("desc") && description.is_none() {
                description = Some(value);
            }
        }
        if !name.is_empty() {
            metrics.push(Metric {
                name: name.to_owned(),
                duration: duration.unwrap_or(0.0),
                description: description.unwrap_or_default(),
            });
        }
    }
}

pub(crate) fn parse(headers: &HeaderMap) -> Vec<Metric> {
    let mut metrics = Vec::new();
    for value in headers.get_all("server-timing") {
        parse_field(value.as_bytes(), &mut metrics);
    }
    metrics
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_fields_and_quoted_parameters_preserve_wire_order() {
        let mut headers = HeaderMap::new();
        headers.append("server-timing", r#"db;DUR="+.5e1ms";desc="read, \"quoted\"";dur=8"#.parse().unwrap());
        headers.append("server-timing", "db;dur=bad;dur=7, app;desc=ready".parse().unwrap());
        let values = parse(&headers);
        assert_eq!(values.len(), 3);
        assert_eq!((&*values[0].name, values[0].duration, &*values[0].description), ("db", 5.0, "read, \"quoted\""));
        assert_eq!((&*values[1].name, values[1].duration), ("db", 0.0));
        assert_eq!((&*values[2].name, &*values[2].description), ("app", "ready"));
        assert_eq!(parse_duration("3e-tail"), 3.0);
        assert_eq!(parse_duration("inf"), 0.0);
    }
}
