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

#[derive(Debug)]
pub struct SvgResource {
    pub bytes: std::sync::Arc<Vec<u8>>,
    pub opaque: bool,
    charge: std::sync::Arc<pyro_raster::SourceLease>,
    pub bindings: Vec<(String, std::sync::Arc<SvgResource>)>,
}
impl SvgResource {
    pub fn new(bytes: impl Into<std::sync::Arc<Vec<u8>>>, opaque: bool, page: &pyro_raster::PageRasters) -> Option<Self> {
        let bytes = bytes.into();
        let charge = page.retain_source(bytes.clone()).ok()?;
        Some(Self { bytes, opaque, charge, bindings: Vec::new() })
    }

    pub fn belongs_to(&self, page: &pyro_raster::PageRasters) -> bool { self.charge.belongs_to(page) }

    /// pyro: HTML-in-Canvas privacy: whether this resource, or one it binds, came from an opaque
    /// response (another origin without CORS): what a canvas a page can read back must not show.
    pub fn is_opaque(&self) -> bool {
        self.opaque || self.bindings.iter().any(|(_, image)| image.is_opaque())
    }

    pub fn retained_bytes(&self) -> usize {
        self.bindings.iter().fold(self.bytes.capacity(), |total, (name, image)| {
            total.saturating_add(name.capacity()).saturating_add(image.bytes.capacity()).saturating_add(std::mem::size_of::<(String, std::sync::Arc<Self>)>())
        })
    }
}
type SvgResourceEntries = std::collections::HashMap<String, std::sync::Arc<SvgResource>>;

/// The SVG image resources a document's `<svg>`s resolve hrefs against, by href. pyro: it counts its
/// changes ([`Self::generation`]: a write guard adds one as it is released), which what is derived
/// from it (`BaseDocument::svg_images_for_paint`) is kept by.
#[derive(Default)]
pub struct SvgResourceMap {
    entries: std::sync::RwLock<SvgResourceEntries>,
    generation: std::sync::atomic::AtomicU64,
}

impl std::fmt::Debug for SvgResourceMap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SvgResourceMap").field("generation", &self.generation()).finish_non_exhaustive()
    }
}

impl SvgResourceMap {
    pub fn new(entries: SvgResourceEntries) -> Self {
        Self { entries: std::sync::RwLock::new(entries), generation: Default::default() }
    }

    pub fn read(&self) -> std::sync::LockResult<std::sync::RwLockReadGuard<'_, SvgResourceEntries>> {
        self.entries.read()
    }

    /// The resources for changing: the map counts the change when the guard is dropped, while the
    /// lock is still held, so a reader that sees a generation sees the resources it counts.
    pub fn write(&self) -> std::sync::LockResult<SvgResourceWrite<'_>> {
        let wrap = |guard| SvgResourceWrite { guard, generation: &self.generation };
        match self.entries.write() {
            Ok(guard) => Ok(wrap(guard)),
            Err(poisoned) => Err(std::sync::PoisonError::new(wrap(poisoned.into_inner()))),
        }
    }

    /// How many times the resources were changed (a write guard was released).
    pub fn generation(&self) -> u64 {
        self.generation.load(std::sync::atomic::Ordering::Acquire)
    }
}

pub struct SvgResourceWrite<'a> {
    guard: std::sync::RwLockWriteGuard<'a, SvgResourceEntries>,
    generation: &'a std::sync::atomic::AtomicU64,
}

impl std::ops::Deref for SvgResourceWrite<'_> {
    type Target = SvgResourceEntries;
    fn deref(&self) -> &SvgResourceEntries { &self.guard }
}

impl std::ops::DerefMut for SvgResourceWrite<'_> {
    fn deref_mut(&mut self) -> &mut SvgResourceEntries { &mut self.guard }
}

impl Drop for SvgResourceWrite<'_> {
    fn drop(&mut self) {
        self.generation.fetch_add(1, std::sync::atomic::Ordering::Release);
    }
}
pub type SvgResources = std::sync::Arc<SvgResourceMap>;

#[derive(Debug)]
pub struct SvgSource<T: ?Sized> {
    value: Box<T>,
    _charge: pyro_raster::Reservation,
}

impl<T: ?Sized> std::ops::Deref for SvgSource<T> {
    type Target = T;
    fn deref(&self) -> &T { &self.value }
}

impl<T: ?Sized> SvgSource<T> where for<'a> Box<T>: From<&'a T> {
    pub(crate) fn copy(value: &T, page: &pyro_raster::PageRasters) -> Option<std::sync::Arc<Self>> {
        let charge = page.reserve(std::mem::size_of_val(value) + std::mem::size_of::<Self>()).ok()?;
        Some(std::sync::Arc::new(Self { value: value.into(), _charge: charge }))
    }
}

pub struct SvgSprite {
    pub local: String,
    pub source: std::sync::Arc<SvgResource>,
    pub prefix: std::ops::Range<usize>,
    pub suffix: std::ops::Range<usize>,
    pub charge: pyro_raster::Reservation,
}

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
#[cfg(all(test, feature = "svg"))]
pub fn parse_svg_image(source: &[u8]) -> Result<crate::node::SvgImageData, usvg::Error> {
    parse_svg_resources(source, None, &Default::default())
}

#[cfg(feature = "svg")]
pub fn parse_svg_image_for_page(source: &[u8], page: &pyro_raster::PageRasters) -> Result<crate::node::SvgImageData, usvg::Error> {
    parse_svg_resources(source, None, page)
}

#[cfg(feature = "svg")]
type RasterConstructor = fn(Arc<Vec<u8>>) -> usvg::ImageKind;

#[cfg(feature = "svg")]
pub(crate) fn bounded_svgz(data: &[u8], page: &pyro_raster::PageRasters) -> Result<(Vec<u8>, pyro_raster::Reservation), usvg::Error> {
    use std::io::Read;
    let footer = data.last_chunk::<4>().ok_or(usvg::Error::MalformedGZip)?;
    let length = u32::from_le_bytes(*footer) as usize;
    if length > 8 << 20 { return Err(usvg::Error::ElementsLimitReached); }
    let charge = page.reserve(length).map_err(|_| usvg::Error::ElementsLimitReached)?;
    let mut decoder = flate2::read::GzDecoder::new(data);
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(length).map_err(|_| usvg::Error::ElementsLimitReached)?;
    let mut chunk = [0; 8192];
    loop {
        let count = decoder.read(&mut chunk).map_err(|_| usvg::Error::MalformedGZip)?;
        if count == 0 { break; }
        if count > length - bytes.len() { return Err(usvg::Error::MalformedGZip); }
        bytes.extend_from_slice(&chunk[..count]);
    }
    if bytes.len() != length || bytes.starts_with(&[0x1f, 0x8b]) { return Err(usvg::Error::MalformedGZip); }
    Ok((bytes, charge))
}

#[cfg(feature = "svg")]
fn svg_data_kind(mime: &str, bytes: Arc<Vec<u8>>, options: &usvg::Options, page: &pyro_raster::PageRasters, owners: &std::sync::Mutex<Vec<Arc<SvgResource>>>, depth: usize) -> Option<usvg::ImageKind> {
    let nested = mime == "image/svg+xml" || (mime == "text/plain" && !matches!(image::guess_format(&bytes), Ok(image::ImageFormat::Png | image::ImageFormat::WebP | image::ImageFormat::Jpeg | image::ImageFormat::Gif)));
    if !nested { return usvg::ImageHrefResolver::default_data_resolver()(mime, bytes, options); }
    if depth >= 16 { return None; }
    let expanded;
    let source = if bytes.starts_with(&[0x1f, 0x8b]) {
        expanded = bounded_svgz(&bytes, page).ok()?;
        expanded.0.as_slice()
    } else { bytes.as_slice() };
    let _scratch = page.reserve(source.len().checked_mul(3)?).ok()?;
    let inline = bind_inline_rasters(source, page).ok()?;
    let nested_options = usvg::Options {
        fontdb: options.fontdb.clone(), dpi: options.dpi, font_family: options.font_family.clone(), font_size: options.font_size,
        languages: options.languages.clone(), shape_rendering: options.shape_rendering, text_rendering: options.text_rendering,
        image_rendering: options.image_rendering, default_size: options.default_size,
        image_href_resolver: usvg::ImageHrefResolver {
            resolve_data: Box::new(|mime, bytes, opt| {
                let resource = Arc::new(SvgResource::new(bytes, false, page)?);
                let kind = svg_data_kind(mime, resource.bytes.clone(), opt, page, owners, depth + 1)?;
                if !matches!(kind, usvg::ImageKind::SVG(_)) { owners.lock().ok()?.push(resource); }
                Some(kind)
            }),
            resolve_string: Box::new(|href, _| {
                let (_, prefix, handles, _) = inline.as_ref()?;
                let index = href.strip_prefix(prefix)?.parse::<usize>().ok()?;
                let (resource, kind) = handles.get(index)?;
                Some(kind(resource.bytes.clone()))
            }),
        }, ..Default::default()
    };
    let text = inline.as_ref().map_or(source, |entry| entry.0.as_bytes());
    let checked = crate::svg_refs::break_reference_cycles(text);
    let tree = usvg::Tree::from_str(std::str::from_utf8(&checked).ok()?, &nested_options).ok()?;
    drop(nested_options);
    if let Some((_, _, handles, _)) = inline { owners.lock().ok()?.extend(handles.into_iter().map(|entry| entry.0)); }
    Some(usvg::ImageKind::SVG(tree))
}

#[cfg(feature = "svg")]
fn bind_inline_rasters(source: &[u8], page: &pyro_raster::PageRasters) -> Result<Option<(String, String, Vec<(Arc<SvgResource>, RasterConstructor)>, pyro_raster::Reservation)>, usvg::Error> {
    if !source.windows(5).any(|v| v.eq_ignore_ascii_case(b"data:")) { return Ok(None); }
    let text = std::str::from_utf8(source).map_err(|_| usvg::Error::NotAnUtf8Str)?;
    let doc = usvg::roxmltree::Document::parse_with_options(text, usvg::roxmltree::ParsingOptions { allow_dtd: true, ..Default::default() })?;
    let mut prefix = [0; 16];
    getrandom::fill(&mut prefix).map_err(|_| usvg::Error::ElementsLimitReached)?;
    let prefix = format!("pyro-inline-{:032x}:", u128::from_ne_bytes(prefix));
    let mut output = String::new();
    let mut pos = 0;
    let mut handles = Vec::new();
    let mut output_charge = None;
    for node in doc.descendants().filter(|node| node.is_element() && node.tag_name().namespace() == Some("http://www.w3.org/2000/svg") && matches!(node.tag_name().name(), "image" | "feImage")) {
        for href in node.attributes().filter(|attr| attr.name() == "href" && matches!(attr.namespace(), None | Some("http://www.w3.org/1999/xlink"))) {
            let range = href.range_value();
            if range.start < pos || range.len() < prefix.len() + 20 { continue; }
            let Ok(url) = data_url::DataUrl::process(href.value()) else { continue };
            let mime = url.mime_type();
            if mime.type_ != "image" || !matches!(mime.subtype.as_str(), "png" | "jpeg" | "jpg" | "webp" | "gif") { continue; }
            if output_charge.is_none() {
                output_charge = Some(page.reserve(text.len()).map_err(|_| usvg::Error::ElementsLimitReached)?);
                output.try_reserve_exact(text.len()).map_err(|_| usvg::Error::ElementsLimitReached)?;
            }
            output.push_str(&text[pos..range.start]);
            pos = range.end;
            let mut length = 0usize;
            if url.decode(|chunk| { length = length.checked_add(chunk.len()).ok_or(())?; Ok::<_, ()>(()) }).is_err() { continue; }
            let Ok(charge) = page.reserve(length) else { continue };
            let mut bytes = Vec::new();
            if bytes.try_reserve_exact(length).is_err() { continue; }
            if url.decode(|chunk| { bytes.extend_from_slice(chunk); Ok::<_, ()>(()) }).is_err() { continue; }
            let kind: RasterConstructor = match mime.subtype.as_str() {
                "png" => usvg::ImageKind::PNG, "webp" => usvg::ImageKind::WEBP, "gif" => usvg::ImageKind::GIF, _ => usvg::ImageKind::JPEG,
            };
            use std::fmt::Write;
            drop(charge);
            let Some(resource) = SvgResource::new(bytes, false, page) else { continue };
            write!(output, "{prefix}{}", handles.len()).unwrap();
            handles.push((Arc::new(resource), kind));
        }
    }
    let Some(charge) = output_charge else { return Ok(None) };
    output.push_str(&text[pos..]);
    Ok(Some((output, prefix, handles, charge)))
}

#[cfg(feature = "svg")]
pub fn prepare_sprite(source: SvgResource, page: &pyro_raster::PageRasters) -> Option<SvgResource> {
    let Some((text, prefix, handles, temporary)) = bind_inline_rasters(&source.bytes, page).ok()? else { return Some(source) };
    drop(source);
    let bytes: Box<str> = text.into_boxed_str();
    drop(temporary);
    let mut prepared = SvgResource::new(String::from(bytes).into_bytes(), false, page)?;
    prepared.bindings = handles.into_iter().enumerate().map(|(index, (image, _))| (format!("{prefix}{index}"), image)).collect();
    Some(prepared)
}

#[cfg(feature = "svg")]
pub fn parse_svg_resources(source: &[u8], resources: Option<&SvgResources>, page: &pyro_raster::PageRasters) -> Result<crate::node::SvgImageData, usvg::Error> {
    let expanded;
    let source = if source.starts_with(&[0x1f, 0x8b]) {
        expanded = bounded_svgz(source, page)?;
        expanded.0.as_slice()
    } else { source };
    // pyro: parsed images by their source. An inline <svg> is
    // parsed from a snapshot of its subtree each time its box is built, and
    // pages that clone content (a header logo on every printed page) build
    // the same one again and again; the parse depends on the source alone.
    std::thread_local! {
        static PARSED: std::cell::RefCell<std::collections::HashMap<[u8; 32], (std::sync::Weak<crate::node::SvgTree>, crate::node::SvgIntrinsicDimensions, u64)>> =
            Default::default();
    }
    const MAX_ENTRIES: usize = 256;
    // pyro: parsed with the fonts loaded so far (a web font loaded since changes the text).
    let (fontdb, generation) = svg_font_db();
    let resources = resources.filter(|map| !map.read().unwrap().is_empty());
    let cacheable = resources.is_none() && source.len() <= 65536 && !source.windows(5).any(|text| text.eq_ignore_ascii_case(b"data:")) && !source.starts_with(&[0x1f, 0x8b]);
    use sha2::Digest;
    let cache_key = cacheable.then(|| <[u8; 32]>::from(sha2::Sha256::digest(source)));
    if let Some(key) = &cache_key && let Some((tree, intrinsic_dimensions)) = PARSED.with(|parsed| {
        let parsed = parsed.borrow();
        let (tree, dimensions, at) = parsed.get(key)?;
        let tree = tree.upgrade()?;
        (*at == generation && tree.page.same_page(page)).then_some((tree, *dimensions))
    }) {
        return Ok(crate::node::SvgImageData { tree, intrinsic_dimensions, resources: None, inline_source: None, source: None, foreign_source: None, viewport_dependent: false });
    }
    let external_images = std::sync::Mutex::new(Vec::new());
    let embedded = std::sync::Mutex::new(Vec::new());
    let _scratch = page.reserve(source.len().checked_mul(3).ok_or(usvg::Error::ElementsLimitReached)?).map_err(|_| usvg::Error::ElementsLimitReached)?;
    let inline = bind_inline_rasters(source, page)?;
    let options = usvg::Options {
        fontdb,
        image_href_resolver: usvg::ImageHrefResolver {
            resolve_data: Box::new(|mime, bytes, options| {
                let charge = page.reserve(bytes.capacity()).ok()?;
                let kind = svg_data_kind(mime, bytes, options, page, &external_images, 0)?;
                if !matches!(kind, usvg::ImageKind::SVG(_)) {
                    embedded.lock().ok()?.push(charge);
                }
                Some(kind)
            }),
            resolve_string: Box::new(|href, options| {
                if let Some((_, prefix, handles, _)) = &inline && let Some(key) = href.strip_prefix(prefix.as_str()) {
                    let (resource, kind) = handles.get(key.parse::<usize>().ok()?)?;
                    return Some(kind(resource.bytes.clone()));
                }
                let raw = href.strip_prefix("pyro-unoriented:");
                let resource = resources?.read().ok()?.get(raw.unwrap_or(href))?.clone();
                let mut used = external_images.lock().ok()?;
                if !used.iter().any(|image| Arc::ptr_eq(image, &resource)) { used.push(resource.clone()); }
                drop(used);
                if resource.bytes.starts_with(&[255, 216]) {
                    let (width, height, orientation) = pyro_raster::jpeg_geometry(&resource.bytes).ok()?;
                    if orientation != 1 {
                        let stored = raw.is_some() && !resource.opaque;
                        let swapped = !stored && (5..=8).contains(&orientation);
                        let (outer_width, outer_height) = if swapped { (height, width) } else { (width, height) };
                        let matrix = if stored { pyro_raster::unturn(orientation, width, height) } else { [1., 0., 0., 1., 0., 0.] };
                        let [a, b, c, d, x, y] = matrix;
                        let wrapper = format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="{outer_width}" height="{outer_height}"><image href="resource" width="{width}" height="{height}" transform="matrix({a} {b} {c} {d} {x} {y})"/></svg>"#);
                        let inner = usvg::Options { image_href_resolver: usvg::ImageHrefResolver {
                            resolve_string: Box::new(|_, _| Some(usvg::ImageKind::JPEG(resource.bytes.clone()))), ..Default::default()
                        }, ..Default::default() };
                        return Some(usvg::ImageKind::SVG(usvg::Tree::from_str(&wrapper, &inner).ok()?));
                    }
                }
                svg_data_kind("text/plain", resource.bytes.clone(), options, page, &external_images, 0)
            }),
        },
        ..Default::default()
    };
    // pyro: a reference cycle usvg would follow without end (`crate::svg_refs`).
    let checked = crate::svg_refs::break_reference_cycles(inline.as_ref().map_or(source, |entry| entry.0.as_bytes()));
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
    let mut svg = crate::node::SvgImageData::from_xml_bytes(schemed.as_deref().unwrap_or(defaulted), &options, page)?;
    drop(options);
    let tree = Arc::get_mut(&mut svg.tree).unwrap();
    tree.external_images = external_images.into_inner().unwrap();
    if let Some((_, _, handles, _)) = inline { tree.external_images.extend(handles.into_iter().map(|(resource, _)| resource)); }
    tree.embedded = embedded.into_inner().unwrap();
    tree.page = page.clone();
    svg.resources = resources.map(std::sync::Arc::downgrade);
    let Some(key) = cache_key.filter(|_| svg.tree.embedded.is_empty()) else { return Ok(svg) };
    PARSED.with(|p| {
        let mut p = p.borrow_mut();
        if p.len() >= MAX_ENTRIES {
            p.clear();
        }
        p.insert(key, (Arc::downgrade(&svg.tree), svg.intrinsic_dimensions, generation));
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
    /// pyro: (tests) the child-list steps `Node::hit_inner` takes (lookups, and the entries of the
    /// item sets it makes).
    pub(crate) static HIT_STEPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// pyro: (tests) the ancestors `inline_span_positioned_chain` (and `painted_ancestor`) visit.
    pub(crate) static CHAIN_STEPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// pyro: (tests) the sibling-list entries comparisons of tree order searched
    /// (`precedes_in_tree_order`), and the nodes numbered for tree order (`tree_order_indices`).
    pub(crate) static TREE_ORDER_COMPARISONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    pub(crate) static TREE_ORDER_WALKS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}
