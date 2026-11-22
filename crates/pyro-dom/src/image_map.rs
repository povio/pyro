//! pyro: image maps (HTML § 4.8.14): an `img` with `usemap` hit-tests the
//! `area` elements of the `map` its hash-name reference names, in Blink's
//! terms (`TreeScope::GetImageMap`, `HTMLAreaElement::GetPath`,
//! `ParseHTMLListOfFloatingPointNumbers`).

use crate::dom::Dom;
use crate::node::NodeId;

fn is_html(dom: &Dom, el: NodeId, local: &str) -> bool {
    dom.element(el).is_ok_and(|e| e.is_html() && &*e.name.local == local)
}

/// The `map` an `img`'s `usemap` names (HTML "rules for parsing a
/// hash-name reference"): the text after its first `#`, non-empty, matched
/// case-sensitively against `name` or `id` of the first `map` in tree order
/// of the image's tree. `None` for any other element.
pub(crate) fn image_map(dom: &Dom, img: NodeId) -> Option<NodeId> {
    if !is_html(dom, img, "img") {
        return None;
    }
    let usemap = dom.get_attribute(img, "usemap").ok().flatten()?;
    let name = &usemap[usemap.find('#')? + 1..];
    if name.is_empty() {
        return None;
    }
    let root = dom.root_node(img, false).ok()?;
    dom.descendants(root).ok()?.find(|&el| {
        is_html(dom, el, "map")
            && (dom.get_attribute(el, "name").ok().flatten() == Some(name)
                || dom.get_attribute(el, "id").ok().flatten() == Some(name))
    })
}

/// The `area` of `map` holding point (`x`, `y`) relative to the image's
/// content box (`size`: that box's size), the first in tree order.
pub(crate) fn area_at(dom: &Dom, map: NodeId, x: f64, y: f64, size: (f64, f64)) -> Option<NodeId> {
    dom.descendants(map).ok()?.find(|&el| {
        is_html(dom, el, "area") && {
            let shape = dom.get_attribute(el, "shape").ok().flatten().unwrap_or("");
            let coords = parse_float_list(dom.get_attribute(el, "coords").ok().flatten().unwrap_or(""));
            area_contains(shape, &coords, x, y, size)
        }
    })
}

fn is_delimiter(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | b'\x0c' | b'\r' | b',' | b';')
}

/// HTML "rules for parsing a list of floating-point numbers" as Blink runs
/// them: delimiters (HTML spaces, `,`, `;`) separate tokens, garbage before
/// a number start (digit, `.`, `-`) is skipped, a token's longest numeric
/// prefix is its value (0 when it has none), trailing delimiters add none.
pub(crate) fn parse_float_list(input: &str) -> Vec<f64> {
    let s = input.as_bytes();
    let mut numbers = Vec::new();
    let mut i = 0;
    while i < s.len() && is_delimiter(s[i]) {
        i += 1;
    }
    while i < s.len() {
        while i < s.len() && !is_delimiter(s[i]) && !(s[i].is_ascii_digit() || s[i] == b'.' || s[i] == b'-') {
            i += 1;
        }
        let start = i;
        while i < s.len() && !is_delimiter(s[i]) {
            i += 1;
        }
        numbers.push(number_prefix(&input[start..i]));
        while i < s.len() && is_delimiter(s[i]) {
            i += 1;
        }
    }
    numbers
}

/// The value of `token`'s longest prefix that is a decimal number
/// (`-`? digits? (`.` digits?)? with at least one digit, then an exponent
/// only when digits follow its `e`), 0 without one or when not finite.
fn number_prefix(token: &str) -> f64 {
    let s = token.as_bytes();
    let mut i = usize::from(s.first() == Some(&b'-'));
    let digits = |i: &mut usize| {
        let from = *i;
        while *i < s.len() && s[*i].is_ascii_digit() {
            *i += 1;
        }
        *i - from
    };
    let mut mantissa = digits(&mut i);
    if s.get(i) == Some(&b'.') {
        let mut j = i + 1;
        let fraction = digits(&mut j);
        if mantissa + fraction > 0 {
            mantissa += fraction;
            i = j;
        }
    }
    if mantissa == 0 {
        return 0.0;
    }
    if matches!(s.get(i), Some(b'e' | b'E')) {
        let mut j = i + 1;
        if matches!(s.get(j), Some(b'+' | b'-')) {
            j += 1;
        }
        if digits(&mut j) > 0 {
            i = j;
        }
    }
    token[..i].parse::<f64>().ok().filter(|v| v.is_finite()).unwrap_or(0.0)
}

/// Whether an area with `shape` (ASCII case-insensitive; missing or invalid
/// is a rectangle) and `coords` holds (`x`, `y`). Too few coordinates, or a
/// circle without a positive radius, hold nothing; extra ones are ignored.
/// A point on a shape's edge is inside (Skia `SkPath::contains`).
pub(crate) fn area_contains(shape: &str, c: &[f64], x: f64, y: f64, size: (f64, f64)) -> bool {
    let is = |names: &[&str]| names.iter().any(|n| shape.eq_ignore_ascii_case(n));
    if is(&["default"]) {
        x >= 0.0 && y >= 0.0 && x < size.0 && y < size.1
    } else if is(&["circle", "circ"]) {
        c.len() >= 3 && c[2] > 0.0 && (x - c[0]).powi(2) + (y - c[1]).powi(2) <= c[2] * c[2]
    } else if is(&["poly", "polygon"]) {
        if c.len() < 6 {
            return false;
        }
        // Even-odd rule over the points (an odd last coordinate dropped).
        let points: Vec<(f64, f64)> = c.chunks_exact(2).map(|p| (p[0], p[1])).collect();
        let mut inside = false;
        let mut j = points.len() - 1;
        for (i, &(xi, yi)) in points.iter().enumerate() {
            let (xj, yj) = points[j];
            let on_edge = (x - xi) * (yj - yi) == (y - yi) * (xj - xi)
                && x >= xi.min(xj)
                && x <= xi.max(xj)
                && y >= yi.min(yj)
                && y <= yi.max(yj);
            if on_edge {
                return true;
            }
            if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
                inside = !inside;
            }
            j = i;
        }
        inside
    } else {
        c.len() >= 4 && {
            let (x0, x1) = (c[0].min(c[2]), c[0].max(c[2]));
            let (y0, y1) = (c[1].min(c[3]), c[1].max(c[3]));
            x >= x0 && x <= x1 && y >= y0 && y <= y1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn float_lists_parse_as_blink() {
        let cases: &[(&str, &[f64])] = &[
            ("2,2,10,10", &[2.0, 2.0, 10.0, 10.0]),
            ("2;2;10;10", &[2.0, 2.0, 10.0, 10.0]),
            ("2\x0c2\n10\r10", &[2.0, 2.0, 10.0, 10.0]),
            ("2\u{b}2\u{b}10\u{b}10", &[2.0]),
            ("2\u{2000}2", &[2.0]),
            ("2a2b20c20,2,10,10", &[2.0, 2.0, 10.0, 10.0]),
            ("2:2:20:20,2,10,10", &[2.0, 2.0, 10.0, 10.0]),
            (",2,2,10,10", &[2.0, 2.0, 10.0, 10.0]),
            ("2,2,10,", &[2.0, 2.0, 10.0]),
            ("2,2,10 ", &[2.0, 2.0, 10.0]),
            ("2%,2%,10%,10%", &[2.0, 2.0, 10.0, 10.0]),
            ("2in,2in,10cm,10cm", &[2.0, 2.0, 10.0, 10.0]),
            ("1.4,.4,1e1,-10", &[1.4, 0.4, 10.0, -10.0]),
            ("='2,2,10,10' ", &[2.0, 2.0, 10.0, 10.0]),
            ("\u{201c}2,2,10,10\"", &[2.0, 2.0, 10.0, 10.0]),
            ("2,2,10ls/spain/10/Canary+Islands.html", &[2.0, 2.0, 10.0]),
            ("2,,10,10", &[2.0, 10.0, 10.0]),
            (",,2;,;2,;,10 \t\r\n10;;", &[2.0, 2.0, 10.0, 10.0]),
            ("", &[]),
            ("-,.,1e,1e+", &[0.0, 0.0, 1.0, 1.0]),
        ];
        for (input, want) in cases {
            assert_eq!(&parse_float_list(input), want, "{input:?}");
        }
    }

    #[test]
    fn shapes() {
        let size = (300.0, 300.0);
        assert!(area_contains("", &[2.0, 2.0, 10.0, 10.0], 3.0, 3.0, size));
        assert!(!area_contains("rect", &[2.0, 2.0, 10.0, 10.0], 1.0, 3.0, size));
        assert!(area_contains("foobar", &[10.0, 10.0, 2.0, 2.0], 9.0, 9.0, size));
        assert!(!area_contains("rect", &[2.0, 2.0, 10.0], 3.0, 3.0, size));
        assert!(area_contains("DEFAULT", &[], 299.0, 299.0, size));
        assert!(area_contains("CIRC", &[20.0, 40.0, 10.0], 11.0, 40.0, size));
        assert!(!area_contains("circle", &[20.0, 40.0, 10.0], 12.0, 32.0, size));
        assert!(!area_contains("circle", &[20.0, 40.0, 0.0], 20.0, 40.0, size));
        assert!(!area_contains("c\u{130}rcle", &[20.0, 40.0, 10.0], 20.0, 40.0, size));
        let star = [100.0, 100.0, 200.0, 100.0, 100.0, 200.0, 150.0, 50.0, 200.0, 200.0];
        assert!(area_contains("poly", &star, 101.0, 101.0, size));
        assert!(area_contains("polygon", &star, 150.0, 51.0, size));
        assert!(!area_contains("poly", &star, 150.0, 125.0, size));
        assert!(area_contains("poly", &[100.0, 100.0, 120.0, 100.0, 100.0, 120.0, 300.0], 101.0, 119.0, size));
        assert!(area_contains("poly", &[100.0, 100.0, 120.0, 100.0, 100.0, 120.0], 119.0, 101.0, size));
        assert!(!area_contains("poly", &[100.0, 100.0, 120.0, 100.0, 100.0, 120.0], 118.0, 118.0, size));
        assert!(!area_contains("poly", &[100.0, 100.0, 120.0, 100.0, 100.0], 101.0, 101.0, size));
    }
}
