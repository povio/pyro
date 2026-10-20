//! Media-query evaluation for `window.matchMedia()` and `@media` rules.
//!
//! Port of upstream `src/browser/css/MediaQuery.zig` (lightpanda-io/browser
//! @ 2ad38773) and of the `Viewport` struct in `src/browser/Viewport.zig`.
//! The scanner (comment handling, top-level comma split, `not`/`only`/`and`,
//! media types, `device-*` aliases, fail-closed parsing) follows
//! MediaQuery.zig:48-343 line by line.
//!
//! Deliberate deviations from upstream, each required by the rewrite's spec:
//! - Lengths accept a decimal part (`37.5rem`). Upstream's `parseLengthPx`
//!   (MediaQuery.zig:348-372) only takes integers, so `(min-width: 37.5rem)`
//!   evaluated to false at every viewport width, which broke the TOS e2e suite.
//!   The upstream test "decimal lengths are rejected" (MediaQuery.zig:633)
//!   therefore does not hold here.
//! - `prefers-color-scheme`, `prefers-reduced-motion`, `hover`, `any-hover`,
//!   `pointer` and `any-pointer` are evaluated against the [`Viewport`]
//!   emulation fields. Upstream reports them as unknown features, which are
//!   false (MediaQuery.zig:531-538). The defaults match desktop Chrome: light,
//!   no-preference, hover, fine.
//!
//! Units are still only `px`, `em`, `rem` (1em = 1rem = 16px) and a bare `0`;
//! there is still no range syntax (`width >= 600px`). Both fail closed, as
//! upstream.

/// `prefers-color-scheme` value.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub enum ColorScheme {
    #[default]
    Light,
    Dark,
}

/// `pointer` / `any-pointer` value.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub enum Pointer {
    None,
    Coarse,
    #[default]
    Fine,
}

/// `screen.orientation.type`.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum OrientationType {
    PortraitPrimary,
    PortraitSecondary,
    LandscapePrimary,
    LandscapeSecondary,
}

impl OrientationType {
    /// The `ScreenOrientation.type` string.
    pub fn as_str(self) -> &'static str {
        match self {
            OrientationType::PortraitPrimary => "portrait-primary",
            OrientationType::PortraitSecondary => "portrait-secondary",
            OrientationType::LandscapePrimary => "landscape-primary",
            OrientationType::LandscapeSecondary => "landscape-secondary",
        }
    }
}

/// An emulated screen orientation (CDP `Emulation.ScreenOrientation`).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ScreenOrientation {
    pub kind: OrientationType,
    /// Degrees: 0, 90, 180 or 270.
    pub angle: u16,
}

/// The emulated output device. Port of upstream `Viewport.zig:19-44` plus the
/// emulation fields CDP's `Emulation.setEmulatedMedia` and
/// `Emulation.setTouchEmulationEnabled` feed.
#[derive(Clone, Debug, PartialEq)]
pub struct Viewport {
    /// CSS px. `window.innerWidth`, `(width)`.
    pub width: u32,
    /// CSS px. `window.innerHeight`, `(height)`.
    pub height: u32,
    /// CSS px to device px (`window.devicePixelRatio`). Upstream `scale`.
    pub device_scale_factor: f64,
    /// `window.screen` size; `None` means the viewport size.
    pub screen_width: Option<u32>,
    pub screen_height: Option<u32>,
    pub color_scheme: ColorScheme,
    /// `prefers-reduced-motion: reduce`.
    pub reduced_motion: bool,
    /// `hover: hover` (primary input can hover).
    pub hover: bool,
    pub pointer: Pointer,
    /// Emulated `screen.orientation` (upstream `Viewport.orientation`). `None`
    /// means the screen never rotates: its type follows the screen dimensions
    /// and its angle is 0. The CSS `(orientation)` feature always follows the
    /// viewport and ignores this.
    pub orientation: Option<ScreenOrientation>,
}

impl Default for Viewport {
    /// Upstream `Viewport.default` (Viewport.zig:41-44): 1920x1080 at scale 1.
    fn default() -> Self {
        Viewport {
            width: 1920,
            height: 1080,
            device_scale_factor: 1.0,
            screen_width: None,
            screen_height: None,
            color_scheme: ColorScheme::Light,
            reduced_motion: false,
            hover: true,
            pointer: Pointer::Fine,
            orientation: None,
        }
    }
}

impl Viewport {
    pub fn new(width: u32, height: u32) -> Self {
        Viewport { width, height, ..Default::default() }
    }
}

/// Returns true if `query` matches `viewport`. Comma-separated queries are
/// evaluated independently and combined with OR (MediaQuery.zig:48-67).
pub fn matches(query: &str, viewport: &Viewport) -> bool {
    let s = query.as_bytes();
    if has_unterminated_comment(s) {
        return false;
    }
    let mut rest = trim_ws_and_comments(s);
    if rest.is_empty() {
        return false;
    }
    loop {
        let cut = next_top_level_comma(rest);
        let piece = trim_ws_and_comments(&rest[..cut]);
        if !piece.is_empty() && matches_single(piece, viewport) {
            return true;
        }
        if cut == rest.len() {
            break;
        }
        rest = &rest[cut + 1..];
        if rest.is_empty() {
            break;
        }
    }
    false
}

fn find_from(s: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    if from > s.len() {
        return None;
    }
    s[from..].windows(needle.len()).position(|w| w == needle).map(|p| p + from)
}

fn is_ws(c: u8) -> bool {
    // Zig std.ascii.isWhitespace: space, \t, \n, \r, \x0B, \x0C
    matches!(c, b' ' | b'\t' | b'\n' | b'\r' | 0x0B | 0x0C)
}

/// MediaQuery.zig:72-83.
fn has_unterminated_comment(s: &[u8]) -> bool {
    let mut i = 0;
    while i + 1 < s.len() {
        if s[i] == b'/' && s[i + 1] == b'*' {
            match find_from(s, i + 2, b"*/") {
                Some(close) => {
                    i = close + 2;
                    continue;
                }
                None => return true,
            }
        }
        i += 1;
    }
    false
}

/// MediaQuery.zig:88-99.
fn skip_ws_and_comments(s: &[u8], start: usize) -> usize {
    let mut i = start;
    while i < s.len() {
        if is_ws(s[i]) {
            i += 1;
        } else if i + 1 < s.len() && s[i] == b'/' && s[i + 1] == b'*' {
            match find_from(s, i + 2, b"*/") {
                Some(close) => i = close + 2,
                None => return s.len(),
            }
        } else {
            break;
        }
    }
    i
}

/// MediaQuery.zig:103-123.
fn trim_ws_and_comments(s: &[u8]) -> &[u8] {
    let mut start = 0;
    let mut end = s.len();
    while start < end {
        if is_ws(s[start]) {
            start += 1;
        } else if start + 1 < end && s[start] == b'/' && s[start + 1] == b'*' {
            match find_from(&s[..end], start + 2, b"*/") {
                Some(close) => start = close + 2,
                None => return &s[start..end],
            }
        } else {
            break;
        }
    }
    while end > start {
        if is_ws(s[end - 1]) {
            end -= 1;
        } else if end >= start + 2 && s[end - 1] == b'/' && s[end - 2] == b'*' {
            let hay = &s[start..end - 2];
            match hay.windows(2).rposition(|w| w == b"/*") {
                Some(open_rel) => end = start + open_rel,
                None => return &s[start..end],
            }
        } else {
            break;
        }
    }
    &s[start..end]
}

/// MediaQuery.zig:127-139.
fn index_of_skipping_comments(s: &[u8], needle: u8) -> Option<usize> {
    let mut i = 0;
    while i < s.len() {
        if i + 1 < s.len() && s[i] == b'/' && s[i + 1] == b'*' {
            let close = find_from(s, i + 2, b"*/")?;
            i = close + 2;
            continue;
        }
        if s[i] == needle {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// MediaQuery.zig:141-161.
fn next_top_level_comma(s: &[u8]) -> usize {
    let mut depth = 0usize;
    let mut i = 0;
    while i < s.len() {
        if i + 1 < s.len() && s[i] == b'/' && s[i + 1] == b'*' {
            match find_from(s, i + 2, b"*/") {
                Some(close) => {
                    i = close + 2;
                    continue;
                }
                None => return s.len(),
            }
        }
        match s[i] {
            b'(' => depth += 1,
            b')' => depth = depth.saturating_sub(1),
            b',' if depth == 0 => return i,
            _ => {}
        }
        i += 1;
    }
    s.len()
}

#[derive(Copy, Clone, PartialEq, Eq)]
enum MediaType {
    All,
    Screen,
    Print,
    Speech,
    Tv,
}

fn parse_media_type(word: &str) -> Option<MediaType> {
    Some(match word {
        "all" => MediaType::All,
        "screen" => MediaType::Screen,
        "print" => MediaType::Print,
        "speech" => MediaType::Speech,
        "tv" => MediaType::Tv,
        _ => return None,
    })
}

fn is_ident_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'-' || c == b'_'
}

/// MediaQuery.zig:174-237.
fn matches_single(query: &[u8], viewport: &Viewport) -> bool {
    let mut negate = false;
    let mut type_set: Option<MediaType> = None;
    let mut features_match = true;
    let mut saw_token = false;

    let mut i = 0;
    while i < query.len() {
        i = skip_ws_and_comments(query, i);
        if i >= query.len() {
            break;
        }
        if query[i] == b'(' {
            let Some(end) = find_closing_paren(query, i) else { return false };
            if !eval_feature(&query[i + 1..end], viewport) {
                features_match = false;
            }
            saw_token = true;
            i = end + 1;
            continue;
        }
        let start = i;
        while i < query.len() && is_ident_char(query[i]) {
            i += 1;
        }
        let len = i - start;
        if !(2..=16).contains(&len) {
            return false;
        }
        let word = String::from_utf8_lossy(&query[start..i]).to_ascii_lowercase();
        saw_token = true;
        match word.as_str() {
            "not" => negate = true,
            // 'only' is a hint to legacy parsers: a no-op qualifier.
            "only" => {}
            // separator between media-type and feature, or between features.
            "and" => {}
            w => match parse_media_type(w) {
                Some(t) => type_set = Some(t),
                // Unknown ident in media-type position: the whole query is a
                // non-matching type.
                None => return false,
            },
        }
    }
    if !saw_token {
        return false;
    }
    let type_matches = match type_set {
        Some(MediaType::All | MediaType::Screen) | None => true,
        Some(MediaType::Print | MediaType::Speech | MediaType::Tv) => false,
    };
    let result = type_matches && features_match;
    if negate {
        !result
    } else {
        result
    }
}

/// MediaQuery.zig:239-258.
fn find_closing_paren(s: &[u8], open: usize) -> Option<usize> {
    let mut depth = 1usize;
    let mut i = open + 1;
    while i < s.len() {
        if i + 1 < s.len() && s[i] == b'/' && s[i + 1] == b'*' {
            let close = find_from(s, i + 2, b"*/")?;
            i = close + 2;
            continue;
        }
        if s[i] == b'(' {
            depth += 1;
        } else if s[i] == b')' {
            depth -= 1;
            if depth == 0 {
                return Some(i);
            }
        }
        i += 1;
    }
    None
}

/// MediaQuery.zig:264-275.
fn eval_feature(text: &[u8], viewport: &Viewport) -> bool {
    let trimmed = trim_ws_and_comments(text);
    if trimmed.is_empty() {
        return false;
    }
    if let Some(colon) = index_of_skipping_comments(trimmed, b':') {
        let name = trim_ws_and_comments(&trimmed[..colon]);
        let value = trim_ws_and_comments(&trimmed[colon + 1..]);
        return eval_name_value(name, value, viewport);
    }
    if let Some(result) = eval_range(trimmed, viewport) {
        return result;
    }
    eval_boolean(trimmed, viewport)
}

#[derive(Copy, Clone)]
enum RangeOp {
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
}

impl RangeOp {
    fn holds(self, a: f64, b: f64) -> bool {
        match self {
            RangeOp::Lt => a < b,
            RangeOp::Le => a <= b,
            RangeOp::Gt => a > b,
            RangeOp::Ge => a >= b,
            RangeOp::Eq => a == b,
        }
    }
}

/// Media Queries 4 range syntax for `width`/`height`: `(width >= 48rem)`,
/// `(48rem <= width)` and `(40rem <= width < 64rem)`. `None` when the text has
/// no comparison operator; a malformed range is `Some(false)`.
fn eval_range(text: &[u8], viewport: &Viewport) -> Option<bool> {
    let mut parts = Vec::new();
    let mut ops = Vec::new();
    let (mut i, mut start) = (0, 0);
    while i < text.len() {
        let op = match text[i] {
            b'<' | b'>' => {
                let inclusive = text.get(i + 1) == Some(&b'=');
                let op = match (text[i], inclusive) {
                    (b'<', false) => RangeOp::Lt,
                    (b'<', true) => RangeOp::Le,
                    (b'>', false) => RangeOp::Gt,
                    _ => RangeOp::Ge,
                };
                Some((op, if inclusive { 2 } else { 1 }))
            }
            b'=' => Some((RangeOp::Eq, 1)),
            _ => None,
        };
        match op {
            Some((op, len)) => {
                parts.push(trim_ws_and_comments(&text[start..i]));
                ops.push(op);
                i += len;
                start = i;
            }
            None => i += 1,
        }
    }
    if ops.is_empty() {
        return None;
    }
    parts.push(trim_ws_and_comments(&text[start..]));
    let feature = |part: &[u8]| -> Option<f64> {
        match strip_device(&lower(part)?) {
            "width" => Some(viewport.width as f64),
            "height" => Some(viewport.height as f64),
            _ => None,
        }
    };
    let length = |part: &[u8]| parse_length_px(&String::from_utf8_lossy(part));
    let compare = |left: &[u8], op: RangeOp, right: &[u8]| -> Option<bool> {
        match (feature(left), feature(right)) {
            (Some(f), None) => Some(op.holds(f, length(right)?)),
            (None, Some(f)) => Some(op.holds(length(left)?, f)),
            _ => None,
        }
    };
    let result = match (parts.as_slice(), ops.as_slice()) {
        ([a, b], [op]) => compare(a, *op, b),
        // `a < name < b` needs both operators in the same direction.
        ([a, name, b], [first @ (RangeOp::Lt | RangeOp::Le), second @ (RangeOp::Lt | RangeOp::Le)])
        | ([a, name, b], [first @ (RangeOp::Gt | RangeOp::Ge), second @ (RangeOp::Gt | RangeOp::Ge)])
            if feature(name).is_some() =>
        {
            Some(compare(a, *first, name)? && compare(name, *second, b)?)
        }
        _ => None,
    };
    Some(result.unwrap_or(false))
}

/// Upstream caps feature names at 17 bytes (MediaQuery.zig:277); the
/// emulation features added here need up to 22 (`prefers-reduced-motion`).
const MAX_FEATURE_NAME_LEN: usize = 22;

/// MediaQuery.zig:283-291: the deprecated `device-*` features map onto their
/// viewport counterparts (the screen reports the viewport size).
fn strip_device(lname: &str) -> &str {
    match lname {
        "device-width" => "width",
        "min-device-width" => "min-width",
        "max-device-width" => "max-width",
        "device-height" => "height",
        "min-device-height" => "min-height",
        "max-device-height" => "max-height",
        other => other,
    }
}

fn lower(s: &[u8]) -> Option<String> {
    if s.len() > MAX_FEATURE_NAME_LEN {
        return None;
    }
    Some(String::from_utf8_lossy(s).to_ascii_lowercase())
}

/// MediaQuery.zig:293-330, plus the emulation features (see module docs).
fn eval_name_value(name: &[u8], value: &[u8], viewport: &Viewport) -> bool {
    let Some(lname) = lower(name) else { return false };
    let lname = strip_device(&lname);
    let w = viewport.width as f64;
    let h = viewport.height as f64;
    let value_str = String::from_utf8_lossy(value);
    let v = value_str.as_ref();
    let px = || parse_length_px(v);
    match lname {
        "min-width" => px().is_some_and(|p| w >= p),
        "max-width" => px().is_some_and(|p| w <= p),
        "width" => px().is_some_and(|p| w == p),
        "min-height" => px().is_some_and(|p| h >= p),
        "max-height" => px().is_some_and(|p| h <= p),
        "height" => px().is_some_and(|p| h == p),
        "orientation" => {
            if v.eq_ignore_ascii_case("landscape") {
                viewport.width >= viewport.height
            } else if v.eq_ignore_ascii_case("portrait") {
                viewport.height > viewport.width
            } else {
                false
            }
        }
        // Emulation features (deviation, see module docs).
        "prefers-color-scheme" => {
            if v.eq_ignore_ascii_case("light") {
                viewport.color_scheme == ColorScheme::Light
            } else if v.eq_ignore_ascii_case("dark") {
                viewport.color_scheme == ColorScheme::Dark
            } else {
                false
            }
        }
        "prefers-reduced-motion" => {
            if v.eq_ignore_ascii_case("reduce") {
                viewport.reduced_motion
            } else if v.eq_ignore_ascii_case("no-preference") {
                !viewport.reduced_motion
            } else {
                false
            }
        }
        "hover" | "any-hover" => {
            if v.eq_ignore_ascii_case("hover") {
                viewport.hover
            } else if v.eq_ignore_ascii_case("none") {
                !viewport.hover
            } else {
                false
            }
        }
        "pointer" | "any-pointer" => {
            let want = if v.eq_ignore_ascii_case("fine") {
                Pointer::Fine
            } else if v.eq_ignore_ascii_case("coarse") {
                Pointer::Coarse
            } else if v.eq_ignore_ascii_case("none") {
                Pointer::None
            } else {
                return false;
            };
            viewport.pointer == want
        }
        _ => false,
    }
}

/// MediaQuery.zig:332-343, plus the emulation features' boolean context.
fn eval_boolean(name: &[u8], viewport: &Viewport) -> bool {
    let Some(lname) = lower(name) else { return false };
    match strip_device(&lname) {
        "width" => viewport.width > 0,
        "height" => viewport.height > 0,
        "orientation" => true,
        "prefers-color-scheme" => true,
        "prefers-reduced-motion" => viewport.reduced_motion,
        "hover" | "any-hover" => viewport.hover,
        "pointer" | "any-pointer" => viewport.pointer != Pointer::None,
        _ => false,
    }
}

/// Port of MediaQuery.zig:348-372 (`<n>px`, `<n>em`, `<n>rem` with 1em=16px,
/// or bare `0`; negatives rejected), extended to accept a decimal part.
fn parse_length_px(value: &str) -> Option<f64> {
    let t = value.trim_matches(|c: char| c.is_ascii() && is_ws(c as u8));
    let b = t.as_bytes();
    if b.is_empty() || b[0] == b'-' {
        return None;
    }
    let mut i = 0;
    if b[i] == b'+' {
        i += 1;
    }
    let num_start = i;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    let int_digits = i - num_start;
    let mut frac_digits = 0;
    if i < b.len() && b[i] == b'.' {
        let dot = i;
        i += 1;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        frac_digits = i - dot - 1;
        if frac_digits == 0 {
            return None;
        }
    }
    if int_digits == 0 && frac_digits == 0 {
        return None;
    }
    let num: f64 = t[num_start..i].parse().ok()?;
    if !num.is_finite() {
        return None;
    }
    let unit = t[i..].trim_matches(|c: char| c.is_ascii() && is_ws(c as u8));
    if unit.is_empty() {
        return (num == 0.0).then_some(0.0);
    }
    if unit.eq_ignore_ascii_case("px") {
        Some(num)
    } else if unit.eq_ignore_ascii_case("em") || unit.eq_ignore_ascii_case("rem") {
        Some(num * 16.0)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_syntax() {
        let v = Viewport::new(1440, 900);
        assert!(matches("(width >= 48rem)", &v));
        assert!(!matches("(width < 48rem)", &v));
        assert!(matches("(48rem <= width)", &v));
        assert!(matches("(width > 1439px)", &v));
        assert!(!matches("(width > 1440px)", &v));
        assert!(matches("(width = 1440px)", &v));
        assert!(matches("(40rem <= width < 96rem)", &v));
        assert!(!matches("(40rem <= width < 80rem)", &v));
        assert!(matches("(height <= 900px)", &v));
        assert!(matches("not all and (width < 40rem)", &v));
        assert!(!matches("(40rem < width > 80rem)", &v));
        assert!(!matches("(width >= nonsense)", &v));
        assert!(!matches("(colour >= 1px)", &v));
    }

    #[test]
    fn decimal_rem_breakpoint() {
        let v = Viewport::new(1440, 900);
        assert!(matches("(min-width: 37.5rem)", &v));
        assert!(matches("(min-width: 90rem)", &v));
        assert!(!matches("(min-width: 90.0625rem)", &v));
        assert!(!matches("(min-width: 1.rem)", &v));
    }
}
