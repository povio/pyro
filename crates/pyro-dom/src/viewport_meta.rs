//! Mobile viewport emulation (CDP `Emulation.setDeviceMetricsOverride` with
//! `mobile: true`, Playwright's `isMobile`): the document's `<meta
//! name="viewport">` decides its layout viewport and page scale, as in
//! Chromium. Ported from Blink: `HTMLMetaElement::ParseViewportContentAttribute`
//! and `GetViewportDescriptionFromContentAttribute` (the tag, with its 0.25 to
//! 5 scale limits), `ViewportStyleResolver` (980 CSS px without a tag),
//! `ViewportDescription::Resolve` (the layout size and the scale limits),
//! `PageScaleConstraints(Set)` (DevTools' mobile limits, 0.25 to 5, and the
//! minimum scale that fits the document's width) and `WebViewImpl::MainFrameSize`
//! (the frame, `innerWidth`/`innerHeight`, is the device at the minimum
//! scale). The arithmetic is Blink's `f32` in CSS px, so sizes round as
//! Chrome's do: a Pixel 7 (412 px) without a tag is 981 px in `innerWidth`
//! and 980 px in `clientWidth`, as in Chrome.

/// Which `<meta>` gave a document its viewport, in increasing priority: a
/// lower kind never replaces a higher one (Blink's `ViewportDescription::Type`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum MetaViewportKind {
    /// `<meta name="HandheldFriendly" content="true">`: `width=device-width`.
    HandheldFriendly,
    /// `<meta name="MobileOptimized">`: `width=device-width, initial-scale=1`.
    MobileOptimized,
    /// `<meta name="viewport" content>`.
    Viewport,
}

/// The viewport `<meta>` a document processed last (Blink's "legacy viewport
/// description"): its kind and the content it stands for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MetaViewport {
    pub kind: MetaViewportKind,
    pub content: Box<str>,
}

impl MetaViewport {
    /// What a `<meta>` named `name` with `content` sets, if anything (Blink's
    /// `HTMLMetaElement::ProcessContent`; names are ASCII case-insensitive).
    pub fn from_meta(name: &str, content: &str) -> Option<MetaViewport> {
        let (kind, content) = if name.eq_ignore_ascii_case("viewport") {
            (MetaViewportKind::Viewport, content)
        } else if name.eq_ignore_ascii_case("handheldfriendly") && content.eq_ignore_ascii_case("true") {
            (MetaViewportKind::HandheldFriendly, "width=device-width")
        } else if name.eq_ignore_ascii_case("mobileoptimized") {
            (MetaViewportKind::MobileOptimized, "width=device-width, initial-scale=1")
        } else {
            return None;
        };
        Some(MetaViewport { kind, content: content.into() })
    }
}

/// Blink's `kValueAuto`.
const AUTO: f32 = -1.0;
/// Blink's `kValueExtendToZoom`.
const EXTEND_TO_ZOOM: f32 = -10.0;
/// The layout width of a page without a tag (Blink's mobile viewport style).
const DEFAULT_MIN_WIDTH: f32 = 980.0;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Length {
    Auto,
    Fixed(f32),
    DeviceWidth,
    DeviceHeight,
    ExtendToZoom,
}

/// Blink's `ViewportDescription`.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Description {
    /// From a `<meta>` (Blink's legacy types), not the user agent's default.
    meta: bool,
    min_width: Length,
    max_width: Length,
    min_height: Length,
    max_height: Length,
    zoom: f32,
    min_zoom: f32,
    max_zoom: f32,
    user_zoom: bool,
}

impl Description {
    /// The user agent's description of a page without a tag
    /// (`ViewportStyleResolver`, mobile style): at least 980 px wide.
    fn user_agent() -> Description {
        Description { meta: false, min_width: Length::Fixed(DEFAULT_MIN_WIDTH), ..Description::empty_meta() }
    }

    fn empty_meta() -> Description {
        Description {
            meta: true,
            min_width: Length::Auto,
            max_width: Length::Auto,
            min_height: Length::Auto,
            max_height: Length::Auto,
            zoom: AUTO,
            min_zoom: AUTO,
            max_zoom: AUTO,
            user_zoom: true,
        }
    }

    /// `HTMLMetaElement::GetViewportDescriptionFromContentAttribute`: the
    /// key-value pairs as Blink tokenizes them (after Internet Explorer: `=`,
    /// `,`, space, tab, CR, LF and NUL separate; `;` separates too, with a
    /// console warning in Chrome), then the default scale limits.
    fn parse(content: &str) -> Description {
        let buf = content.to_ascii_lowercase().into_bytes();
        let len = buf.len();
        // WTF::String's operator[] answers 0 past the end.
        let at = |i: usize| buf.get(i).copied().unwrap_or(0);
        let separator = |c: u8| matches!(c, b' ' | b'\t' | b'\n' | b'\r' | b'=' | b',' | 0);
        let mut d = Description::empty_meta();
        let mut i = 0;
        while i < len {
            while separator(at(i)) && i < len {
                i += 1;
            }
            let key_begin = i;
            while !separator(at(i)) && i < len {
                i += 1;
            }
            let key_end = i;
            while at(i) != b'=' && at(i) != b',' && i < len {
                i += 1;
            }
            while separator(at(i)) && at(i) != b',' && i < len {
                i += 1;
            }
            let value_begin = i;
            while !separator(at(i)) && i < len {
                i += 1;
            }
            d.set(&buf[key_begin..key_end], &buf[value_begin..i]);
        }
        if d.min_zoom == AUTO {
            d.min_zoom = 0.25;
        }
        if d.max_zoom == AUTO {
            d.max_zoom = 5.0;
            d.min_zoom = d.min_zoom.min(5.0);
        }
        d
    }

    /// `HTMLMetaElement::ProcessViewportKeyValuePair` (lowercase key and value).
    fn set(&mut self, key: &[u8], value: &[u8]) {
        match key {
            b"width" => {
                let width = length(value);
                if width != Length::Auto {
                    self.min_width = Length::ExtendToZoom;
                    self.max_width = width;
                }
            }
            b"height" => {
                let height = length(value);
                if height != Length::Auto {
                    self.min_height = Length::ExtendToZoom;
                    self.max_height = height;
                }
            }
            b"initial-scale" => self.zoom = zoom(value),
            b"minimum-scale" => self.min_zoom = zoom(value),
            b"maximum-scale" => self.max_zoom = zoom(value),
            b"user-scalable" => self.user_zoom = user_zoom(value),
            // `target-densitydpi` (Android WebView only), `viewport-fit`,
            // `interactive-widget`, `minimal-ui`, `shrink-to-fit`, unknown
            // keys: no layout effect in Chrome.
            _ => {}
        }
    }

    /// `ViewportDescription::Resolve` for a device `initial` CSS px wide and
    /// high: the layout size and the page-defined scale limits.
    fn resolve(&self, initial: (f32, f32)) -> Constraints {
        let (w, h) = initial;
        let (mut copy_min_width, mut copy_max_width) = (self.min_width, self.max_width);
        if self.meta && self.max_width == Length::Auto {
            if self.zoom == AUTO {
                copy_min_width = Length::ExtendToZoom;
                copy_max_width = Length::Fixed(DEFAULT_MIN_WIDTH);
            } else if self.max_height == Length::Auto {
                copy_min_width = Length::ExtendToZoom;
                copy_max_width = Length::ExtendToZoom;
            }
        }
        let resolve = |l: Length| match l {
            Length::Auto => AUTO,
            Length::Fixed(v) => v,
            Length::ExtendToZoom => EXTEND_TO_ZOOM,
            Length::DeviceWidth => w,
            Length::DeviceHeight => h,
        };
        let mut max_width = resolve(copy_max_width);
        let mut min_width = resolve(copy_min_width);
        let mut max_height = resolve(self.max_height);
        let mut min_height = resolve(self.min_height);
        let (mut zoom, mut min_zoom, mut max_zoom) = (self.zoom, self.min_zoom, self.max_zoom);
        if min_zoom != AUTO && max_zoom != AUTO {
            max_zoom = min_zoom.max(max_zoom);
        }
        if zoom != AUTO {
            zoom = or_auto(min_zoom, or_auto(max_zoom, zoom, f32::min), f32::max);
        }
        let extend_zoom = or_auto(zoom, max_zoom, f32::min);
        if extend_zoom == AUTO {
            if max_width == EXTEND_TO_ZOOM {
                max_width = AUTO;
            }
            if max_height == EXTEND_TO_ZOOM {
                max_height = AUTO;
            }
            if min_width == EXTEND_TO_ZOOM {
                min_width = max_width;
            }
            if min_height == EXTEND_TO_ZOOM {
                min_height = max_height;
            }
        } else {
            let (extend_width, extend_height) = (w / extend_zoom, h / extend_zoom);
            if max_width == EXTEND_TO_ZOOM {
                max_width = extend_width;
            }
            if max_height == EXTEND_TO_ZOOM {
                max_height = extend_height;
            }
            if min_width == EXTEND_TO_ZOOM {
                min_width = or_auto(extend_width, max_width, f32::max);
            }
            if min_height == EXTEND_TO_ZOOM {
                min_height = or_auto(extend_height, max_height, f32::max);
            }
        }
        let mut width = AUTO;
        if min_width != AUTO || max_width != AUTO {
            width = or_auto(min_width, or_auto(max_width, w, f32::min), f32::max);
        }
        let mut height = AUTO;
        if min_height != AUTO || max_height != AUTO {
            height = or_auto(min_height, or_auto(max_height, h, f32::min), f32::max);
        }
        if width == AUTO {
            width = if height == AUTO || h == 0.0 { w } else { height * (w / h) };
        }
        if height == AUTO {
            height = if w == 0.0 { h } else { width * h / w };
        }
        if zoom == AUTO {
            if width != AUTO && width > 0.0 {
                zoom = w / width;
            }
            if height != AUTO && height > 0.0 {
                zoom = zoom.max(h / height);
            }
            zoom = or_auto(min_zoom, or_auto(max_zoom, zoom, f32::min), f32::max);
        }
        if !self.user_zoom {
            min_zoom = zoom;
            max_zoom = zoom;
        }
        if self.zoom == AUTO {
            zoom = AUTO;
        }
        Constraints { initial: zoom, minimum: min_zoom, maximum: max_zoom, layout: (width, height) }
    }
}

/// Blink's `CompareIgnoringAuto`.
fn or_auto(a: f32, b: f32, compare: fn(f32, f32) -> f32) -> f32 {
    if a == AUTO {
        b
    } else if b == AUTO {
        a
    } else {
        compare(a, b)
    }
}

/// `HTMLMetaElement::ParseViewportValueAsLength`: a number of CSS px (1 to
/// 10000), `device-width`, `device-height`; negative and unknown values are
/// `auto`.
fn length(value: &[u8]) -> Length {
    match value {
        b"device-width" => Length::DeviceWidth,
        b"device-height" => Length::DeviceHeight,
        _ => match number(value) {
            Some(v) if v >= 0.0 => Length::Fixed(v.clamp(1.0, 10000.0)),
            _ => Length::Auto,
        },
    }
}

/// `HTMLMetaElement::ParseViewportValueAsZoom`: `yes` is 1, `no` 0,
/// `device-width` and `device-height` 10, a number is clamped to 0.1 to 10,
/// a negative one is `auto` and anything else 0.1.
fn zoom(value: &[u8]) -> f32 {
    match value {
        b"yes" => 1.0,
        b"no" => 0.0,
        b"device-width" | b"device-height" => 10.0,
        _ => {
            let v = number(value).unwrap_or(0.0);
            if v < 0.0 {
                AUTO
            } else {
                v.clamp(0.1, 10.0)
            }
        }
    }
}

/// `HTMLMetaElement::ParseViewportValueAsUserZoom`.
fn user_zoom(value: &[u8]) -> bool {
    match value {
        b"yes" | b"device-width" | b"device-height" => true,
        b"no" => false,
        _ => number(value).unwrap_or(0.0).abs() >= 1.0,
    }
}

/// The number a value starts with (`CharactersToFloat`, trailing junk
/// allowed: `2.5px` is 2.5), as Blink's `f32`; `None` without one.
fn number(value: &[u8]) -> Option<f32> {
    let digits = |from: usize| value[from.min(value.len())..].iter().take_while(|c| c.is_ascii_digit()).count();
    let start = value.iter().take_while(|&&c| matches!(c, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r')).count();
    let mut end = start + usize::from(matches!(value.get(start), Some(b'+' | b'-')));
    let whole = digits(end);
    end += whole;
    let mut fraction = 0;
    if value.get(end) == Some(&b'.') {
        fraction = digits(end + 1);
        if whole + fraction > 0 {
            end += 1 + fraction;
        }
    }
    if whole + fraction == 0 {
        return None;
    }
    if matches!(value.get(end), Some(b'e' | b'E')) {
        let sign = usize::from(matches!(value.get(end + 1), Some(b'+' | b'-')));
        let exponent = digits(end + 1 + sign);
        if exponent > 0 {
            end += 1 + sign + exponent;
        }
    }
    let text = std::str::from_utf8(&value[start..end]).ok()?;
    text.parse::<f64>().ok().map(|v| v as f32)
}

/// Blink's `PageScaleConstraints`: `AUTO` (-1) for an unset scale.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Constraints {
    initial: f32,
    minimum: f32,
    maximum: f32,
    layout: (f32, f32),
}

impl Constraints {
    /// `PageScaleConstraints::OverrideWith`.
    fn override_with(&mut self, other: &Constraints) {
        if other.initial != AUTO {
            self.initial = other.initial;
            if self.minimum != AUTO {
                self.minimum = self.minimum.min(other.initial);
            }
        }
        if other.minimum != AUTO {
            self.minimum = other.minimum;
        }
        if other.maximum != AUTO {
            self.maximum = other.maximum;
        }
        self.layout = other.layout;
        self.clamp_all();
    }

    fn clamp(&self, scale: f32) -> f32 {
        if scale == AUTO {
            return scale;
        }
        let mut s = scale;
        if self.minimum != AUTO {
            s = s.max(self.minimum);
        }
        if self.maximum != AUTO {
            s = s.min(self.maximum);
        }
        s
    }

    fn clamp_all(&mut self) {
        if self.minimum != AUTO && self.maximum != AUTO {
            self.maximum = self.minimum.max(self.maximum);
        }
        self.initial = self.clamp(self.initial);
    }
}

/// A mobile page's viewport before layout: the device's size (CSS px) and
/// what its `<meta name="viewport">` (or the lack of one) makes of it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MobileViewport {
    device: (f32, f32),
    page: Constraints,
}

/// A mobile page's viewport metrics once its document's width is known.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MobileMetrics {
    /// The frame (`innerWidth`, `innerHeight`, the layout viewport of
    /// scrolling and `Page.getLayoutMetrics`): the device at the minimum
    /// scale, rounded up.
    pub frame: (u32, u32),
    /// The visual viewport (`visualViewport.width`/`height`): the device at
    /// the page scale.
    pub visual: (f64, f64),
    /// The page scale (`visualViewport.scale`): the tag's `initial-scale`
    /// within the limits, else the minimum.
    pub scale: f64,
}

impl MobileViewport {
    /// The viewport of a document whose last viewport `<meta>` is `meta`, on
    /// a device `width` x `height` CSS px.
    pub fn new(meta: Option<&MetaViewport>, width: u32, height: u32) -> MobileViewport {
        let description = match meta {
            Some(m) => Description::parse(&m.content),
            None => Description::user_agent(),
        };
        let device = (width as f32, height as f32);
        MobileViewport { device, page: description.resolve(device) }
    }

    /// The layout viewport (initial containing block): what layout, media
    /// queries, viewport units and the root's `clientWidth`/`clientHeight`
    /// see (Blink's `GetLayoutSize`, rounded down; at least 1 px).
    pub fn layout_size(&self) -> (u32, u32) {
        let px = |v: f32| (v.floor() as u32).max(1);
        (px(self.page.layout.0), px(self.page.layout.1))
    }

    /// The metrics of the page laid out with a document `contents_width` CSS
    /// px wide (at least the layout viewport's width): Blink's final scale
    /// limits (DevTools' mobile defaults, 0.25 to 5, under the page's own;
    /// the minimum fits the document's width), the scale it resets to, and
    /// the frame at the minimum scale.
    pub fn metrics(&self, contents_width: f32) -> MobileMetrics {
        let mut c = Constraints { initial: AUTO, minimum: 0.25, maximum: 5.0, layout: self.page.layout };
        c.override_with(&self.page);
        let (w, h) = self.device;
        // `FitToContentsWidth` (the emulation shrinks content to fit).
        if contents_width != 0.0 && w != 0.0 {
            c.minimum = c.minimum.max(w / contents_width);
            c.clamp_all();
        }
        // `ResolveAutoInitialScale`.
        if c.initial == AUTO {
            c.initial = c.minimum;
        }
        c.clamp_all();
        // Blink scales sizes by the reciprocal (`gfx::ScaleToCeiledSize`,
        // `VisibleRect`), in `f32`.
        let at_minimum = 1.0 / c.minimum;
        let at_scale = 1.0 / c.initial;
        MobileMetrics {
            frame: ((w * at_minimum).ceil() as u32, (h * at_minimum).ceil() as u32),
            visual: ((w * at_scale) as f64, (h * at_scale) as f64),
            scale: c.initial as f64,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(content: &str) -> MetaViewport {
        MetaViewport { kind: MetaViewportKind::Viewport, content: content.into() }
    }

    /// Layout size, frame, visual viewport and scale of `content` (`None`:
    /// no tag) on a device, with a document `contents` px wide (0: as wide
    /// as the layout viewport).
    fn resolved(content: Option<&str>, device: (u32, u32), contents: f32) -> ((u32, u32), MobileMetrics) {
        let m = content.map(meta);
        let v = MobileViewport::new(m.as_ref(), device.0, device.1);
        let layout = v.layout_size();
        let contents = if contents == 0.0 { layout.0 as f32 } else { contents.max(layout.0 as f32) };
        (layout, v.metrics(contents))
    }

    const IPHONE_13: (u32, u32) = (390, 664);
    const PIXEL_7: (u32, u32) = (412, 839);

    /// Chrome 147 (Playwright 1.59.1's Chromium, `devices['iPhone 13']` and
    /// `devices['Pixel 7']`): `[clientWidth, clientHeight]`, `[innerWidth,
    /// innerHeight]`, `[visualViewport.width, visualViewport.height,
    /// visualViewport.scale]`.
    #[test]
    fn matches_chrome() {
        #[rustfmt::skip]
        let cases: &[(Option<&str>, (u32, u32), f32, (u32, u32), (u32, u32), (f64, f64, f64))] = &[
            (Some("width=device-width, initial-scale=1"), IPHONE_13, 0.0, (390, 664), (390, 664), (390.0, 664.0, 1.0)),
            (None, IPHONE_13, 0.0, (980, 1668), (980, 1669), (980.0, 1668.5128173828125, 0.3979591727256775)),
            (Some("width=600"), IPHONE_13, 0.0, (600, 1021), (600, 1022), (600.0, 1021.5384521484375, 0.6499999761581421)),
            (Some("initial-scale=2"), IPHONE_13, 0.0, (195, 332), (195, 332), (195.0, 332.0, 2.0)),
            (Some("width=device-width, initial-scale=2"), IPHONE_13, 0.0, (390, 664), (390, 664), (195.0, 332.0, 2.0)),
            (Some("width=device-width"), IPHONE_13, 0.0, (390, 664), (390, 664), (390.0, 664.0, 1.0)),
            // Content wider than the layout viewport widens the frame.
            (Some("width=device-width, initial-scale=1"), IPHONE_13, 1200.0, (390, 664), (1200, 2044), (390.0, 664.0, 1.0)),
            (None, IPHONE_13, 1500.0, (980, 1668), (1500, 2554), (1500.0, 2553.84619140625, 0.25999999046325684)),
            (Some("width=device-width, initial-scale=1, user-scalable=no"), IPHONE_13, 1200.0, (390, 664), (390, 664), (390.0, 664.0, 1.0)),
            (Some("width=device-width, minimum-scale=1"), IPHONE_13, 1200.0, (390, 664), (390, 664), (390.0, 664.0, 1.0)),
            (Some("width=device-width"), IPHONE_13, 1200.0, (390, 664), (1200, 2044), (1200.0, 2043.076904296875, 0.32499998807907104)),
            (Some("width=200"), IPHONE_13, 0.0, (200, 340), (200, 341), (199.99998474121094, 340.5127868652344, 1.9500000476837158)),
            (Some("width=device-width, initial-scale=0.5"), IPHONE_13, 0.0, (780, 1328), (780, 1328), (780.0, 1328.0, 0.5)),
            (Some("width=50000"), IPHONE_13, 0.0, (10000, 17025), (1560, 2656), (1560.0, 2656.0, 0.25)),
            (Some("initial-scale=10"), IPHONE_13, 0.0, (78, 132), (78, 133), (78.0, 132.8000030517578, 5.0)),
            (Some("minimum-scale=2"), IPHONE_13, 0.0, (980, 1668), (195, 332), (195.0, 332.0, 2.0)),
            (Some("maximum-scale=0.5"), IPHONE_13, 0.0, (980, 1668), (980, 1669), (980.0, 1668.5128173828125, 0.3979591727256775)),
            (Some(""), IPHONE_13, 0.0, (980, 1668), (980, 1669), (980.0, 1668.5128173828125, 0.3979591727256775)),
            (Some("width=abc"), IPHONE_13, 0.0, (980, 1668), (980, 1669), (980.0, 1668.5128173828125, 0.3979591727256775)),
            (Some("width=0"), IPHONE_13, 0.0, (78, 132), (78, 133), (78.0, 132.8000030517578, 5.0)),
            (Some("initial-scale=0"), IPHONE_13, 0.0, (1560, 2656), (1560, 2656), (1560.0, 2656.0, 0.25)),
            (Some("initial-scale=abc"), IPHONE_13, 0.0, (1560, 2656), (1560, 2656), (1560.0, 2656.0, 0.25)),
            (Some("height=1000"), IPHONE_13, 0.0, (980, 1000), (980, 1669), (980.0, 1668.5128173828125, 0.3979591727256775)),
            (Some("width=600, initial-scale=1"), IPHONE_13, 0.0, (600, 1021), (600, 1022), (390.0, 664.0, 1.0)),
            (Some("width=1200"), IPHONE_13, 0.0, (1200, 2043), (1200, 2044), (1200.0, 2043.076904296875, 0.32499998807907104)),
            (Some("width=device-width; initial-scale=1"), IPHONE_13, 0.0, (390, 664), (390, 664), (390.0, 664.0, 1.0)),
            (Some("width=device-width, maximum-scale=1, initial-scale=2"), IPHONE_13, 0.0, (390, 664), (390, 664), (390.0, 664.0, 1.0)),
            (Some("width=500"), IPHONE_13, 0.0, (500, 851), (501, 852), (500.0000305175781, 851.2821044921875, 0.7799999713897705)),
            (Some("width=700"), IPHONE_13, 0.0, (700, 1191), (700, 1192), (700.0, 1191.794921875, 0.5571428537368774)),
            (Some("width=device-width, initial-scale=1"), IPHONE_13, 1201.0, (390, 664), (1201, 2045), (390.0, 664.0, 1.0)),
            (Some("initial-scale=no"), IPHONE_13, 0.0, (1560, 2656), (1560, 2656), (1560.0, 2656.0, 0.25)),
            (Some("minimum-scale=0.1, initial-scale=0.1"), IPHONE_13, 0.0, (3900, 6640), (3900, 6640), (3900.0, 6640.0, 0.10000000149011612)),
            (Some("width=device-height"), IPHONE_13, 0.0, (664, 1130), (664, 1131), (663.9999389648438, 1130.50244140625, 0.5873494148254395)),
            // `user-scalable=no` locks the minimum: the frame is narrower than the layout viewport.
            (Some("initial-scale=1, width=400, user-scalable=no"), IPHONE_13, 0.0, (400, 681), (390, 664), (390.0, 664.0, 1.0)),
            (None, PIXEL_7, 0.0, (980, 1995), (981, 1996), (980.0000610351562, 1995.6796875, 0.4204081594944)),
            (Some("width=600"), PIXEL_7, 0.0, (600, 1221), (600, 1222), (600.0, 1221.8446044921875, 0.6866666674613953)),
            (Some("initial-scale=2"), PIXEL_7, 0.0, (206, 419), (206, 420), (206.0, 419.5, 2.0)),
            (Some("width=device-width, initial-scale=1"), PIXEL_7, 1200.0, (412, 839), (1200, 2444), (412.0, 839.0, 1.0)),
        ];
        for &(content, device, contents, layout, frame, (vw, vh, scale)) in cases {
            let (l, m) = resolved(content, device, contents);
            assert_eq!((l, m.frame), (layout, frame), "{content:?} on {device:?}, {contents} px of content");
            assert_eq!(m.visual, (vw, vh), "{content:?} on {device:?}: visual viewport");
            assert_eq!(m.scale, scale, "{content:?} on {device:?}: scale");
        }
    }

    #[test]
    fn legacy_tags_and_priority() {
        let handheld = MetaViewport::from_meta("HandheldFriendly", "TRUE").unwrap();
        assert_eq!(handheld.kind, MetaViewportKind::HandheldFriendly);
        assert_eq!(MobileViewport::new(Some(&handheld), 390, 664).layout_size(), (390, 664));
        assert_eq!(MetaViewport::from_meta("handheldfriendly", "false"), None);
        let optimized = MetaViewport::from_meta("MobileOptimized", "320").unwrap();
        assert_eq!(&*optimized.content, "width=device-width, initial-scale=1");
        assert!(optimized.kind > handheld.kind && meta("x").kind > optimized.kind);
        assert_eq!(MetaViewport::from_meta("VIEWPORT", "width=600").unwrap().kind, MetaViewportKind::Viewport);
        assert_eq!(MetaViewport::from_meta("description", "width=600"), None);
    }

    #[test]
    fn parses_as_blink() {
        let d = Description::parse(" Width = 600 ,INITIAL-SCALE=2.5px,,user-scalable=0.5;foo");
        assert_eq!(
            (d.min_width, d.max_width, d.zoom, d.user_zoom),
            (Length::ExtendToZoom, Length::Fixed(600.0), 2.5, false)
        );
        assert_eq!((d.min_zoom, d.max_zoom), (0.25, 5.0));
        // A key with no `=` has an empty value; a later pair still counts.
        let d = Description::parse("width, initial-scale=3");
        assert_eq!((d.max_width, d.zoom), (Length::Auto, 3.0));
        assert_eq!(Description::parse("user-scalable=no, maximum-scale=1").max_zoom, 1.0);
        // Without a maximum, the minimum is at most the default maximum, 5.
        let d = Description::parse("minimum-scale=7");
        assert_eq!((d.min_zoom, d.max_zoom), (5.0, 5.0));
        let d = Description::parse("minimum-scale=7, maximum-scale=8");
        assert_eq!((d.min_zoom, d.max_zoom), (7.0, 8.0));
        assert_eq!(number(b"-.5e1x"), Some(-5.0));
        assert_eq!((number(b"."), number(b"e3"), number(b"1e")), (None, None, Some(1.0)));
    }
}
