use crate::node::{Node, NodeData};
use color::{AlphaColor, Srgb};
use keyboard_types::Modifiers;
use std::borrow::Cow;
use style::color::AbsoluteColor;

#[cfg(target_os = "macos")]
pub(crate) const ACTION_MOD: Modifiers = Modifiers::META;
#[cfg(not(target_os = "macos"))]
pub(crate) const ACTION_MOD: Modifiers = Modifiers::CONTROL;

pub type Color = AlphaColor<Srgb>;

/// pyro: stack left below which [`ensure_stack`] moves to a new stack segment. It covers
/// everything one tree level runs between two checkpoints: a level of Taffy's and Blitz's layout
/// dispatch (about 3 KB), plus a leaf's own work (inline layout and shaping, table and grid
/// sizing, a paint of text), which is not recursive.
pub const STACK_RED_ZONE: usize = 256 * 1024;
/// pyro: size of each stack segment [`ensure_stack`] maps (lazily committed): about
/// 1,500 more levels of nested blocks per segment.
pub const STACK_GROW_BY: usize = 4 * 1024 * 1024;

/// pyro: run `f`, first moving to a new stack segment when less than
/// [`STACK_RED_ZONE`] is left. Recursive tree walks (layout, paint, the paint tree, damage,
/// rounding, box construction) run each level through this, so a deep tree never overflows the
/// thread's stack and aborts the process, whatever stack the caller left (JS recursion near
/// JavaScriptCore's limit, a 2 MB spawned thread). With room left it costs a thread-local read and
/// one stack-pointer compare.
#[inline(always)]
pub fn ensure_stack<R>(f: impl FnOnce() -> R) -> R {
    stacker::maybe_grow(STACK_RED_ZONE, STACK_GROW_BY, f)
}

/// Decode raw font bytes, decompressing WOFF/WOFF2 if the `woff` feature is enabled.
/// Returns the original slice unchanged for TTF/OTF input, and also on decompression
/// failure. With the `woff` feature disabled, all input passes through unchanged.
pub fn decode_font_bytes(bytes: &[u8]) -> Cow<'_, [u8]> {
    if bytes.len() < 4 {
        return Cow::Borrowed(bytes);
    }
    match &bytes[0..4] {
        #[cfg(feature = "woff")]
        b"wOFF" => wuff::decompress_woff1(bytes)
            .map(Cow::Owned)
            .unwrap_or_else(|_| {
                #[cfg(feature = "tracing")]
                tracing::warn!("Failed to decompress woff1 font");
                Cow::Borrowed(bytes)
            }),
        #[cfg(feature = "woff")]
        b"wOF2" => wuff::decompress_woff2(bytes)
            .map(Cow::Owned)
            .unwrap_or_else(|_| {
                #[cfg(feature = "tracing")]
                tracing::warn!("Failed to decompress woff2 font");
                Cow::Borrowed(bytes)
            }),
        _ => Cow::Borrowed(bytes),
    }
}

#[cfg(feature = "svg")]
use std::sync::{Arc, LazyLock};
#[cfg(feature = "svg")]
use usvg::fontdb;
#[cfg(feature = "svg")]
pub(crate) static FONT_DB: LazyLock<Arc<fontdb::Database>> = LazyLock::new(|| {
    let mut db = fontdb::Database::new();
    db.load_system_fonts();
    // pyro: and the fonts `add_svg_font` gave.
    for font in SVG_FONTS.lock().unwrap_or_else(|e| e.into_inner()).iter() {
        db.load_font_source(fontdb::Source::Binary(font.clone()));
    }
    Arc::new(db)
});

/// pyro: fonts added to usvg's font database when it is made.
#[cfg(feature = "svg")]
static SVG_FONTS: std::sync::Mutex<Vec<Arc<dyn AsRef<[u8]> + Send + Sync>>> = std::sync::Mutex::new(Vec::new());

/// pyro: adds `font` (TTF/OTF/TTC bytes) to the fonts SVG text is drawn with, beside the system
/// fonts, as if it were installed. Call before the first SVG is parsed: the database is made then.
#[cfg(feature = "svg")]
pub fn add_svg_font(font: Arc<dyn AsRef<[u8]> + Send + Sync>) {
    SVG_FONTS.lock().unwrap_or_else(|e| e.into_inner()).push(font);
}

/// Which kind of CSS image layer list (`background-image` or `mask-image`) to
/// flush from style to dedicated storage on the node.
#[derive(Clone, Copy, Debug)]
pub enum ImageLayerKind {
    Background,
    Mask,
    /// pyro: `border-image-source`, a list of one image.
    BorderImage,
}

impl ImageLayerKind {
    pub fn image_type(self, idx: usize) -> ImageType {
        match self {
            Self::Background => ImageType::Background(idx),
            Self::Mask => ImageType::Mask(idx),
            Self::BorderImage => ImageType::BorderImage(idx),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum ImageType {
    Image,
    Background(usize),
    Mask(usize),
    /// pyro: `border-image-source`.
    BorderImage(usize),
}

/// A point
#[derive(Clone, Debug, Copy, Eq, PartialEq)]
pub struct Point<T> {
    /// The x coordinate
    pub x: T,
    /// The y coordinate
    pub y: T,
}

impl Point<f64> {
    pub const ZERO: Self = Point { x: 0.0, y: 0.0 };
}

// Debug print an RcDom
pub fn walk_tree(indent: usize, node: &Node) {
    // Skip all-whitespace text nodes entirely
    if let NodeData::Text(data) = &node.data {
        if data.content.chars().all(|c| c.is_ascii_whitespace()) {
            return;
        }
    }

    print!("{}", " ".repeat(indent));
    let id = node.id;
    match &node.data {
        NodeData::Document(_) => println!("#Document {id}"),

        NodeData::Text(data) => {
            if data.content.chars().all(|c| c.is_ascii_whitespace()) {
                println!("{id} #text: <whitespace>");
            } else {
                let content = data.content.trim();
                if content.len() > 10 {
                    println!(
                        "#text {id}: {}...",
                        content
                            .split_at(content.char_indices().take(10).last().unwrap().0)
                            .0
                            .escape_default()
                    )
                } else {
                    println!("#text {id}: {}", data.content.trim().escape_default())
                }
            }
        }

        NodeData::Comment { .. } => println!("<!-- COMMENT {id} -->"),

        NodeData::AnonymousBlock(_) => println!("{id} AnonymousBlock"),

        NodeData::Element(data) => {
            print!("<{} {id}", data.name.local);
            for attr in data.attrs.iter() {
                print!(" {}=\"{}\"", attr.name.local, attr.value);
            }
            if !node.children.is_empty() {
                println!(">");
            } else {
                println!("/>");
            }
        } // NodeData::Doctype {
          //     ref name,
          //     ref public_id,
          //     ref system_id,
          // } => println!("<!DOCTYPE {} \"{}\" \"{}\">", name, public_id, system_id),
          // NodeData::ProcessingInstruction { .. } => unreachable!(),
    }

    if !node.children.is_empty() {
        for child_id in node.children.iter() {
            walk_tree(indent + 2, node.with(*child_id));
        }

        if let NodeData::Element(data) = &node.data {
            println!("{}</{}>", " ".repeat(indent), data.name.local);
        }
    }
}

/// Parse an SVG image.
#[cfg(feature = "svg")]
pub(crate) fn parse_svg_image(source: &[u8]) -> Result<crate::node::SvgImageData, usvg::Error> {
    // pyro: parsed images by their source. An inline <svg> is
    // parsed from a snapshot of its subtree each time its box is built, and
    // pages that clone content (a header logo on every printed page) build
    // the same one again and again; the parse depends on the source alone.
    std::thread_local! {
        static PARSED: std::cell::RefCell<std::collections::HashMap<Vec<u8>, crate::node::SvgImageData>> =
            Default::default();
    }
    const MAX_ENTRIES: usize = 256;
    if let Some(svg) = PARSED.with(|p| p.borrow().get(source).cloned()) {
        return Ok(svg);
    }
    let options = usvg::Options {
        fontdb: Arc::clone(&*FONT_DB),
        ..Default::default()
    };
    // pyro: a reference cycle usvg would follow without end (`crate::svg_refs`).
    let checked = crate::svg_refs::break_reference_cycles(source);
    // pyro: `feConvolveMatrix divisor="0"` is its default, as in Chromium (`crate::svg_refs`).
    let defaulted = crate::svg_refs::default_zero_divisors(&checked);
    let svg = crate::node::SvgImageData::from_data(defaulted.as_deref().unwrap_or(&checked), &options)?;
    PARSED.with(|p| {
        let mut p = p.borrow_mut();
        if p.len() >= MAX_ENTRIES {
            p.clear();
        }
        p.insert(source.to_vec(), svg.clone());
    });
    Ok(svg)
}

pub trait ToColorColor {
    /// Converts a color into the `AlphaColor<Srgb>` type from the `color` crate
    fn as_color_color(&self) -> Color;
}
impl ToColorColor for AbsoluteColor {
    fn as_color_color(&self) -> Color {
        Color::new(
            *self
                .to_color_space(style::color::ColorSpace::Srgb)
                .raw_components(),
        )
    }
}

#[cfg(all(test, feature = "svg"))]
mod svg_tests {
    use super::parse_svg_image;

    #[test]
    fn svg_without_xmlns_parses() {
        let src = br#"<svg viewBox="0 0 24 24"><path d="M0 0h24v24H0z"/></svg>"#;
        let svg = parse_svg_image(src).unwrap();
        assert!(!svg.tree.root().children().is_empty());
    }

    #[test]
    fn missing_height_is_computed_from_width_and_viewbox_ratio() {
        let src = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100" width="200"><rect width="100%" height="100%" fill="green"/></svg>"#;
        let svg = parse_svg_image(src).unwrap();
        assert_eq!(svg.intrinsic_width(), Some(200.0));
        assert_eq!(svg.intrinsic_height(), None);
        assert_eq!(svg.viewbox_aspect_ratio(), Some(1.0));
        assert_eq!(svg.tree.size().width(), 200.0);
        assert_eq!(svg.intrinsic_size(), (200.0, 200.0));
    }

    #[test]
    fn viewbox_only_has_no_intrinsic_dimensions() {
        let src = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 485 58"></svg>"#;
        let svg = parse_svg_image(src).unwrap();
        assert_eq!(svg.intrinsic_width(), None);
        assert_eq!(svg.intrinsic_height(), None);
        // The aspect ratio is still available from the viewBox.
        assert!((svg.aspect_ratio() - (485.0 / 58.0)).abs() < 1e-3);
    }

    #[test]
    fn absolute_dimensions_are_intrinsic() {
        let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="16" viewBox="0 0 48 32"></svg>"#;
        let svg = parse_svg_image(src).unwrap();
        assert_eq!(svg.intrinsic_width(), Some(24.0));
        assert_eq!(svg.intrinsic_height(), Some(16.0));
    }

    #[test]
    fn percentage_dimensions_are_not_intrinsic() {
        let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100%" height="50%" viewBox="0 0 200 100"></svg>"#;
        let svg = parse_svg_image(src).unwrap();
        assert_eq!(svg.intrinsic_width(), None);
        assert_eq!(svg.intrinsic_height(), None);
    }

    #[test]
    fn unit_lengths_are_intrinsic() {
        let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="24px" height="1.5em" viewBox="0 0 48 32"></svg>"#;
        let svg = parse_svg_image(src).unwrap();
        assert!(svg.intrinsic_width().is_some());
        assert!(svg.intrinsic_height().is_some());
    }

    #[test]
    fn non_numeric_dimensions_are_not_intrinsic() {
        let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="auto" height="foo" viewBox="0 0 200 100"></svg>"#;
        let svg = parse_svg_image(src).unwrap();
        assert_eq!(svg.intrinsic_width(), None);
        assert_eq!(svg.intrinsic_height(), None);
    }
}

/// Creates an markup5ever::QualName.
/// Given a local name and an optional namespace
#[macro_export]
macro_rules! qual_name {
    ($local:tt $(, $ns:ident)?) => {
        $crate::QualName {
            prefix: None,
            ns: $crate::ns!($($ns)?),
            local: $crate::local_name!($local),
        }
    };
}

/// pyro: the font a downloaded (or `data:`) font file becomes,
/// one per distinct file on this thread. Documents that load the same web
/// font then register the same blob (same id), so shaping caches keyed by
/// the font's blob id serve every document; each document still registers
/// it in its own font collection. Bounded at 64 files.
pub(crate) fn intern_font_blob(bytes: blitz_traits::net::Bytes) -> linebender_resource_handle::Blob<u8> {
    use std::hash::{Hash, Hasher};
    std::thread_local! {
        static FONTS: std::cell::RefCell<std::collections::HashMap<u64, Vec<linebender_resource_handle::Blob<u8>>>> =
            Default::default();
    }
    const MAX_FILES: usize = 64;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut h);
    let key = h.finish();
    FONTS.with(|fonts| {
        let mut fonts = fonts.borrow_mut();
        if let Some(blob) = fonts.get(&key).and_then(|v| v.iter().find(|b| b.data() == &bytes[..])) {
            return blob.clone();
        }
        if fonts.values().map(Vec::len).sum::<usize>() >= MAX_FILES {
            fonts.clear();
        }
        let blob = linebender_resource_handle::Blob::new(Arc::new(bytes));
        fonts.entry(key).or_default().push(blob.clone());
        blob
    })
}

thread_local! {
    /// The Stylo lock of this thread's documents (see `thread_style_lock`).
    static STYLE_LOCK: style::shared_lock::SharedRwLock = style::shared_lock::SharedRwLock::new();
    /// Parsed author sheets by text hash: (text, base URL, sheet).
    static SHEETS: std::cell::RefCell<std::collections::HashMap<u64, Vec<(std::sync::Arc<str>, String, style::stylesheets::DocumentStyleSheet)>>> =
        Default::default();
}

/// pyro: the Stylo lock every document of this thread uses, so
/// their sheets and rules can be shared (documents of a thread never style
/// concurrently).
pub(crate) fn thread_style_lock() -> style::shared_lock::SharedRwLock {
    STYLE_LOCK.with(|l| l.clone())
}

fn sheet_key(css: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::hash::DefaultHasher::new();
    css.hash(&mut h);
    h.finish()
}

/// pyro: the sheet parsed earlier on this thread for `css` with
/// base URL `url`, if any.
pub(crate) fn shared_sheet(css: &str, url: &str) -> Option<style::stylesheets::DocumentStyleSheet> {
    let key = sheet_key(css);
    SHEETS.with(|s| {
        s.borrow()
            .get(&key)?
            .iter()
            .find(|(text, base, _)| &**text == css && base == url)
            .map(|(_, _, sheet)| sheet.clone())
    })
}

/// pyro: keeps `sheet` (parsed from `css` with base URL `url`) for
/// later documents; bounded at 64 sheets and 32 MB of text.
pub(crate) fn share_sheet(css: &str, url: &str, sheet: &style::stylesheets::DocumentStyleSheet) {
    const MAX_SHEETS: usize = 64;
    const MAX_BYTES: usize = 32 << 20;
    SHEETS.with(|s| {
        let mut s = s.borrow_mut();
        let (count, bytes) = s.values().flatten().fold((0, 0), |(n, b), (t, _, _)| (n + 1, b + t.len()));
        if count >= MAX_SHEETS || bytes + css.len() > MAX_BYTES {
            s.clear();
        }
        s.entry(sheet_key(css)).or_default().push((css.into(), url.to_owned(), sheet.clone()));
    });
}
