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

/// pyro: the font database SVG text is drawn with: [`FONT_DB`] and the web fonts documents
/// loaded (`@font-face`, `FontFace`), each face under its CSS family name ([`add_web_svg_font`]).
/// `None` until a web font loads. Replaced (copy on write) as fonts come; its generation keys
/// the parsed images ([`parse_svg_image`]), so SVG text drawn before a font loaded is drawn again
/// with it.
#[cfg(feature = "svg")]
static WEB_FONT_DB: std::sync::RwLock<Option<(Arc<fontdb::Database>, u64)>> = std::sync::RwLock::new(None);

/// pyro: the font database SVG text is drawn with now, and its generation (0: the system's).
#[cfg(feature = "svg")]
fn svg_font_db() -> (Arc<fontdb::Database>, u64) {
    match &*WEB_FONT_DB.read().unwrap_or_else(|e| e.into_inner()) {
        Some((db, generation)) => (Arc::clone(db), *generation),
        None => (Arc::clone(&*FONT_DB), 0),
    }
}

/// pyro: a web font's faces (`bytes`) for SVG text, under the CSS `family` name the document
/// loaded it as (usvg matches `font-family` against the database: the font's own name may differ,
/// `DelayedAhem` for Ahem; WPT css-masking `clip-path-svg-text-font-loading`).
#[cfg(feature = "svg")]
pub(crate) fn add_web_svg_font(bytes: &[u8], family: &str) {
    let mut guard = WEB_FONT_DB.write().unwrap_or_else(|e| e.into_inner());
    let (mut db, generation) = match guard.as_ref() {
        Some((db, generation)) => ((**db).clone(), *generation),
        None => ((**FONT_DB).clone(), 0),
    };
    let source = fontdb::Source::Binary(Arc::new(bytes.to_vec()));
    for id in db.load_font_source(source) {
        let Some(mut info) = db.face(id).cloned() else { continue };
        db.remove_face(id);
        info.families = vec![(family.to_string(), fontdb::Language::English_UnitedStates)];
        db.push_face_info(info);
    }
    *guard = Some((Arc::new(db), generation + 1));
}

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
    /// pyro: `list-style-image`, a list of one image.
    ListStyle,
    /// pyro: a `shape-outside` image, a list of one.
    Shape,
}

impl ImageLayerKind {
    pub fn image_type(self, idx: usize) -> ImageType {
        match self {
            Self::Background => ImageType::Background(idx),
            Self::Mask => ImageType::Mask(idx),
            Self::BorderImage => ImageType::BorderImage(idx),
            Self::ListStyle => ImageType::ListStyle(idx),
            Self::Shape => ImageType::Shape(idx),
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
    /// pyro: `list-style-image`.
    ListStyle(usize),
    /// pyro: `shape-outside`.
    Shape(usize),
    /// pyro: an SVG document an element's `clip-path` or `filter` references an element of
    /// (`ElementData::reference_images`).
    Reference(usize),
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

/// pyro: the image a `filter()` image filters (through nested ones) or an `image-set()` selected,
/// else `image` itself: the image whose resource an image layer loads and draws.
pub fn unfiltered_image(image: &style::values::computed::Image) -> &style::values::computed::Image {
    use style::values::generics::image::GenericImage;
    match image {
        GenericImage::Filter(filtered) => unfiltered_image(&filtered.image),
        GenericImage::ImageSet(set) => match set.items.get(set.selected_index) {
            Some(item) => unfiltered_image(&item.image),
            None => image,
        },
        image => image,
    }
}

/// pyro: the images an image layer list (`background-image`, `mask-image`) loads, a slot each in
/// the element's stored images (`ElementData::background_images`, `mask_images`): each layer's
/// image (`unfiltered_image`), then the `url()` images of the layers' `cross-fade()`s (nested ones
/// too), in order. blitz-paint finds a `cross-fade()` argument's image among them by its URL.
pub fn image_slots(images: &[style::values::computed::Image]) -> Vec<&style::values::computed::Image> {
    use style::values::generics::image::{GenericCrossFadeImage, GenericImage};
    fn cross_fade_urls<'a>(image: &'a style::values::computed::Image, out: &mut Vec<&'a style::values::computed::Image>) {
        let GenericImage::CrossFade(cross_fade) = image else { return };
        for element in cross_fade.elements.iter() {
            if let GenericCrossFadeImage::Image(image) = &element.image {
                let image = unfiltered_image(image);
                match image {
                    GenericImage::Url(_) => out.push(image),
                    GenericImage::CrossFade(_) => crate::util::ensure_stack(|| cross_fade_urls(image, out)),
                    _ => {}
                }
            }
        }
    }
    let mut slots: Vec<&style::values::computed::Image> = images.iter().map(unfiltered_image).collect();
    for image in images {
        cross_fade_urls(unfiltered_image(image), &mut slots);
    }
    slots
}

/// pyro: whether `style`'s `content` is one image, which replaces the element's content (CSS
/// Content 3 §2; `BaseDocument::flush_content_image`), unless the element is `display: contents`
/// (WPT css-display `display-contents-replaced-content`).
pub fn content_replaces(style: &style::properties::ComputedValues) -> bool {
    use style::values::computed::{Content, ContentItem};
    !style.get_box().display.is_contents()
        && matches!(&style.get_counters().content, Content::Items(items) if matches!(&items.items[..], [ContentItem::Image(_)]))
}

/// pyro: the gradient `style`'s `content` is when it is one gradient image (through `image-set()`
/// and `filter()`): the box shows it in place of content (CSS Content 3 §2).
pub fn content_gradient(style: &style::properties::ComputedValues) -> Option<&style::values::computed::Gradient> {
    use style::values::computed::{Content, ContentItem};
    use style::values::generics::image::GenericImage;
    if style.get_box().display.is_contents() {
        return None;
    }
    let Content::Items(items) = &style.get_counters().content else { return None };
    let [ContentItem::Image(image)] = &items.items[..] else { return None };
    match unfiltered_image(image) {
        GenericImage::Gradient(gradient) => Some(gradient),
        _ => None,
    }
}

/// pyro: the image pixels per CSS px of `image`: the `<resolution>` of the `image-set()` option
/// selected (through nested ones), 1 for any other image. A raster image's natural size is its
/// pixel size divided by it.
pub fn image_resolution(image: &style::values::computed::Image) -> f32 {
    use style::values::generics::image::GenericImage;
    match image {
        GenericImage::Filter(filtered) => image_resolution(&filtered.image),
        GenericImage::ImageSet(set) => match set.items.get(set.selected_index) {
            Some(item) => {
                let own = item.resolution.dppx();
                (if own > 0.0 { own } else { 1.0 }) * image_resolution(&item.image)
            }
            None => 1.0,
        },
        _ => 1.0,
    }
}

/// pyro: the view box `object-view-box` gives replaced content of the natural size
/// `natural_width` x `natural_height` (CSS px): its left, top, width and height in the content's
/// own space. `None` without the property, and for a box without an area (the property is then
/// ignored, as in Chromium). The view box stands for the content where it is sized and placed:
/// it is the natural size, and what `object-fit` and `object-position` fit to the content box.
pub fn object_view_box(
    style: &style::properties::ComputedValues,
    natural_width: f32,
    natural_height: f32,
) -> Option<(f32, f32, f32, f32)> {
    use style::values::computed::CSSPixelLength;
    use style::values::generics::basic_shape::{GenericBasicShape, GenericClipPath};
    let GenericClipPath::Shape(shape, _) = &style.get_position().object_view_box else {
        return None;
    };
    let GenericBasicShape::Rect(inset) = &**shape else {
        return None;
    };
    // pyro: the insets' lengths are computed, so zoomed (`zoom`), and the natural size is not:
    // resolved against the zoomed size and unzoomed (WPT css-images `object-view-box-zoom`).
    let zoom = style.effective_zoom.value().max(f32::MIN_POSITIVE);
    let (w, h) = (CSSPixelLength::new(natural_width * zoom), CSSPixelLength::new(natural_height * zoom));
    let top = inset.rect.0.resolve(h).px() / zoom;
    let right = inset.rect.1.resolve(w).px() / zoom;
    let bottom = inset.rect.2.resolve(h).px() / zoom;
    let left = inset.rect.3.resolve(w).px() / zoom;
    let (width, height) = (natural_width - left - right, natural_height - top - bottom);
    (width > 0.0 && height > 0.0 && [left, top, width, height].iter().all(|v| v.is_finite()))
        .then_some((left, top, width, height))
}

/// pyro: whether `style` shows images as stored, not as their EXIF orientation says
/// (`image-orientation: none`).
pub fn image_unoriented(style: &style::properties::ComputedValues) -> bool {
    style.get_inherited_box().image_orientation == style::values::computed::ImageOrientation::None
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
pub fn parse_svg_image(source: &[u8]) -> Result<crate::node::SvgImageData, usvg::Error> {
    // pyro: parsed images by their source. An inline <svg> is
    // parsed from a snapshot of its subtree each time its box is built, and
    // pages that clone content (a header logo on every printed page) build
    // the same one again and again; the parse depends on the source alone.
    std::thread_local! {
        static PARSED: std::cell::RefCell<std::collections::HashMap<Vec<u8>, (crate::node::SvgImageData, u64)>> =
            Default::default();
    }
    const MAX_ENTRIES: usize = 256;
    // pyro: parsed with the fonts loaded so far (a web font loaded since changes the text).
    let (fontdb, generation) = svg_font_db();
    if let Some(svg) = PARSED.with(|p| p.borrow().get(source).filter(|(_, at)| *at == generation).map(|(svg, _)| svg.clone())) {
        return Ok(svg);
    }
    let options = usvg::Options {
        fontdb,
        ..Default::default()
    };
    // pyro: a reference cycle usvg would follow without end (`crate::svg_refs`).
    let checked = crate::svg_refs::break_reference_cycles(source);
    // pyro: `feConvolveMatrix divisor="0"` is its default, as in Chromium (`crate::svg_refs`).
    let defaulted = crate::svg_refs::default_zero_divisors(&checked);
    let defaulted = defaulted.as_deref().unwrap_or(&checked);
    // pyro: `light-dark()` is its light colour; an element whose used colour scheme is dark draws
    // [`crate::node::SvgImageData::for_color_scheme`].
    // pyro: its `prefers-color-scheme` media blocks for the light scheme, and `:root`
    // (`crate::svg_refs::resolve_scheme_media`).
    let media = crate::svg_refs::resolve_scheme_media(defaulted, false);
    let defaulted = media.as_deref().unwrap_or(defaulted);
    let schemed = crate::svg_refs::resolve_light_dark(defaulted, false);
    let svg = crate::node::SvgImageData::from_data(schemed.as_deref().unwrap_or(defaulted), &options)?;
    PARSED.with(|p| {
        let mut p = p.borrow_mut();
        if p.len() >= MAX_ENTRIES {
            p.clear();
        }
        p.insert(source.to_vec(), (svg.clone(), generation));
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
///
/// A file is one blob per `@font-face` family name it is loaded under: a run's
/// font then tells which family drew it (`@font-palette-values` rules apply
/// to the fonts of the families they name).
pub(crate) fn intern_font_blob(
    bytes: blitz_traits::net::Bytes,
    family: Option<&str>,
) -> linebender_resource_handle::Blob<u8> {
    use std::hash::{Hash, Hasher};
    std::thread_local! {
        static FONTS: std::cell::RefCell<std::collections::HashMap<u64, Vec<(Option<String>, linebender_resource_handle::Blob<u8>)>>> =
            Default::default();
    }
    const MAX_FILES: usize = 64;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut h);
    family.hash(&mut h);
    let key = h.finish();
    FONTS.with(|fonts| {
        let mut fonts = fonts.borrow_mut();
        if let Some((_, blob)) = fonts
            .get(&key)
            .and_then(|v| v.iter().find(|(f, b)| f.as_deref() == family && b.data() == &bytes[..]))
        {
            return blob.clone();
        }
        if fonts.values().map(Vec::len).sum::<usize>() >= MAX_FILES {
            fonts.clear();
        }
        let blob = linebender_resource_handle::Blob::new(Arc::new(bytes));
        fonts.entry(key).or_default().push((family.map(str::to_owned), blob.clone()));
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

#[cfg(test)]
std::thread_local! {
    /// pyro: (tests) how many boxes `Node::hit_inner` was asked about on this thread.
    pub(crate) static HIT_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// pyro: (tests) the sibling-list entries comparisons of tree order searched
    /// (`precedes_in_tree_order`), and the nodes numbered for tree order (`tree_order_indices`).
    pub(crate) static TREE_ORDER_COMPARISONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    pub(crate) static TREE_ORDER_WALKS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}
