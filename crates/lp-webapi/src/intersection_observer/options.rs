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
    let [top, right, bottom, left] = margin.map(|m| m.pixels(rect.width));
    Rect { x: rect.x - left, y: rect.y - top, width: rect.width + left + right, height: rect.height + top + bottom }
}
fn margins(text: &str) -> JsResult<([Margin; 4], String)> {
    let invalid = || JsError::dom("SyntaxError", "rootMargin requires one to four absolute lengths or percentages");
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
    let result = match parsed.as_slice() {
        [a] => [*a; 4],
        [a, b] => [*a, *b, *a, *b],
        [a, b, c] => [*a, *b, *c, *b],
        [a, b, c, d] => [*a, *b, *c, *d],
        _ => return Err(invalid()),
    };
    let text = result
        .iter()
        .map(|m| format!("{}{}", m.value, if m.percent { "%" } else { "px" }))
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
    if !delay.is_undefined() && delay.to_number()? != 0.0 {
        return Err(JsError::dom("NotSupportedError", "IntersectionObserver delay is not implemented"));
    }
    let root = get("root")?;
    let root = if root.is_null() || root.is_undefined() {
        None
    } else {
        let node = s
            .unwrap::<crate::node::Node>(root)
            .ok_or_else(|| JsError::Type("root must be an Element or Document".into()))?;
        if node.page().id() != page.id() {
            return Err(JsError::dom("NotSupportedError", "Cross-document observer roots are not implemented"));
        }
        let kind = page.dom.borrow().node_type(node.id).js()?;
        if !matches!(kind, lp_dom::NodeType::Element | lp_dom::NodeType::Document) {
            return Err(JsError::Type("root must be an Element or Document".into()));
        }
        Some(NodeLease::new(page, node.id)?)
    };
    let margin = get("rootMargin")?;
    let (margin, margin_text) = margins(&if margin.is_undefined() { "0px".into() } else { margin.to_string()? })?;
    let scroll = get("scrollMargin")?;
    if !scroll.is_undefined() {
        let (values, _) = margins(&scroll.to_string()?)?;
        if values.iter().any(|m| m.value != 0.0) {
            return Err(JsError::dom("NotSupportedError", "Nonzero scrollMargin is not implemented"));
        }
    }
    let threshold = get("threshold")?;
    let helper =
        page.intersections.helpers.borrow().get(&s.realm_id()).map(|p| p.get(s)).ok_or(JsError::ContextDestroyed)?;
    let array =
        helper.as_object().unwrap().get_index(0)?.as_object().unwrap().call(None, &[threshold])?.as_object().unwrap();
    let len = array.get("length")?.to_number()? as u32;
    let thresholds = (0..len).map(|i| array.get_index(i)?.to_number()).collect::<JsResult<Vec<_>>>()?;
    if get("trackVisibility")?.to_bool() {
        return Err(JsError::dom("NotSupportedError", "IntersectionObserver visibility tracking is not implemented"));
    }
    Ok(Options { root, margin, margin_text, thresholds })
}
