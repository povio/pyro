use super::*;

#[derive(Clone, Copy, Default)]
pub(super) struct Margin {
    value: f64,
    percent: bool,
}
pub(super) struct Options {
    pub root: Option<NodeLease>,
    pub margin: [Margin; 4],
    pub margin_text: String,
    pub thresholds: Vec<f64>,
    /// How far each scroll container's clip between the target and the root is expanded
    /// (percentages are of the container's own box), as `rootMargin` expands the root.
    pub scroll_margin: [Margin; 4],
    pub scroll_margin_text: String,
    /// IntersectionObserver v2: `trackVisibility` makes entries report `isVisible`, and `delay` (at
    /// least 100 ms then) is the least time between two notifications.
    pub track_visibility: bool,
    pub delay: f64,
}
impl Margin {
    fn pixels(self, width: f64) -> f64 {
        if self.percent {
            self.value * width / 100.0
        } else {
            self.value
        }
    }
}
pub(super) fn expand(rect: Rect, margin: &[Margin; 4]) -> Rect {
    let [top, right, bottom, left] = *margin;
    let (top, bottom) = (top.pixels(rect.height), bottom.pixels(rect.height));
    let (left, right) = (left.pixels(rect.width), right.pixels(rect.width));
    Rect { x: rect.x - left, y: rect.y - top, width: rect.width + left + right, height: rect.height + top + bottom }
}
fn margins(text: &str, name: &str) -> JsResult<([Margin; 4], String)> {
    let invalid = || JsError::dom("SyntaxError", format!("{name} requires one to four absolute lengths or percentages"));
    let mut parsed = Vec::new();
    for token in text.split_ascii_whitespace() {
        let (number, percent, scale) = if let Some(n) = token.strip_suffix('%') {
            (n, true, 1.0)
        } else {
            let token_lower = token.to_ascii_lowercase();
            let Some((unit, scale)) = [
                ("px", 1.0),
                ("cm", 96.0 / 2.54),
                ("mm", 96.0 / 25.4),
                ("q", 96.0 / 101.6),
                ("in", 96.0),
                ("pt", 96.0 / 72.0),
                ("pc", 16.0),
            ]
            .into_iter()
            .find(|(u, _)| token_lower.ends_with(u)) else {
                return Err(invalid());
            };
            (&token[..token.len() - unit.len()], false, scale)
        };
        let value = number.parse::<f64>().map_err(|_| invalid())? * scale;
        if !value.is_finite() || number.chars().any(|c| c.is_alphabetic() && c != 'e' && c != 'E') {
            return Err(invalid());
        }
        parsed.push(Margin { value, percent });
    }
    // An empty string is a margin of size zero (Chrome; WPT empty-root-margin).
    if parsed.is_empty() {
        parsed.push(Margin::default());
    }
    let result = match parsed.as_slice() {
        [a] => [*a; 4],
        [a, b] => [*a, *b, *a, *b],
        [a, b, c] => [*a, *b, *c, *b],
        [a, b, c, d] => [*a, *b, *c, *d],
        _ => return Err(invalid()),
    };
    let text = result
        .iter()
        // Lengths serialize as whole pixels: `10cm` reads back `377px` (Chrome).
        .map(|m| if m.percent { format!("{}%", m.value) } else { format!("{}px", m.value as i64) })
        .collect::<Vec<_>>()
        .join(" ");
    Ok((result, text))
}
pub(super) fn parse(s: &Scope<'_>, page: &Rc<PageDom>, value: Value<'_>) -> JsResult<Options> {
    let object = if value.is_null() || value.is_undefined() {
        None
    } else {
        Some(
            value
                .as_object()
                .ok_or_else(|| JsError::Type("IntersectionObserver options must be a dictionary".into()))?,
        )
    };
    let get = |name| object.map_or(Ok(s.undefined()), |o| o.get(name));
    let delay = get("delay")?;
    let delay = if delay.is_undefined() { 0.0 } else { delay.to_number()? };
    let delay = if delay.is_finite() { delay.trunc() } else { 0.0 };
    let root = get("root")?;
    let root = if root.is_null() || root.is_undefined() {
        None
    } else {
        let node = s
            .unwrap::<crate::node::Node>(root)
            .ok_or_else(|| JsError::Type("root must be an Element or Document".into()))?;
        // The root may belong to another document (an iframe's, from its parent): its lease and
        // geometry are that document's.
        let root_page = node.page().clone();
        let kind = root_page.dom.borrow().node_type(node.id).js()?;
        if !matches!(kind, pyro_dom::NodeType::Element | pyro_dom::NodeType::Document) {
            return Err(JsError::Type("root must be an Element or Document".into()));
        }
        Some(NodeLease::new(&root_page, node.id)?)
    };
    let margin = get("rootMargin")?;
    let (margin, margin_text) = margins(&if margin.is_undefined() { "0px".into() } else { margin.to_string()? }, "rootMargin")?;
    let scroll = get("scrollMargin")?;
    let (scroll_margin, scroll_margin_text) = margins(&if scroll.is_undefined() { "0px".into() } else { scroll.to_string()? }, "scrollMargin")?;
    let threshold = get("threshold")?;
    let helper =
        page.intersections.helpers.borrow().get(&s.realm_id()).map(|p| p.get(s)).ok_or(JsError::ContextDestroyed)?;
    let array =
        helper.as_object().unwrap().get_index(0)?.as_object().unwrap().call(None, &[threshold])?.as_object().unwrap();
    let len = array.get_atom("length")?.to_number()? as u32;
    let thresholds = (0..len).map(|i| array.get_index(i)?.to_number()).collect::<JsResult<Vec<_>>>()?;
    let track_visibility = get("trackVisibility")?.to_bool();
    // A tracking observer notifies at most every 100 ms (IntersectionObserver v2).
    let delay = if track_visibility { delay.max(100.0) } else { delay };
    Ok(Options { root, margin, margin_text, thresholds, scroll_margin, scroll_margin_text, track_visibility, delay })
}

/// HTML's lazy load root margin, as Chromium's (1250 px around the viewport and each
/// scroll container on the way).
pub(super) fn lazy_load() -> Options {
    const MARGIN: f64 = 1250.0;
    Options {
        root: None,
        margin: [Margin { value: MARGIN, percent: false }; 4],
        margin_text: String::new(),
        thresholds: vec![0.0],
        scroll_margin: [Margin { value: MARGIN, percent: false }; 4],
        scroll_margin_text: String::new(),
        track_visibility: false,
        delay: 0.0,
    }
}
