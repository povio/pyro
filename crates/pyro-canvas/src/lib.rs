//! Canvas 2D bitmaps: a [`Surface`] is one canvas's output bitmap, painted
//! with vello_cpu, plus the parts of the HTML canvas drawing model vello_cpu
//! has no notion of (compositing operators other than source-over, shadows,
//! the clipping region as a mask, pixel readback and writes).
//!
//! - The bitmap is allocated on the first draw or pixel write; a surface
//!   that is only read stays unallocated (reads are transparent black).
//! - Source-over draws without a shadow are batched into one vello scene and
//!   rasterized onto the bitmap when pixels are needed (a read, a snapshot,
//!   or a draw that cannot join the batch): source-over is associative, so a
//!   batch composited at once equals the draws composited one by one.
//! - Other draws render the source image alone (full surface), then
//!   composite it onto the bitmap per pixel with the operator, the global
//!   alpha and the clipping region's coverage ([`composite`]); a shadow is
//!   made from the source image's alpha, offset and blurred, and composited
//!   first. Its source is rendered apart, offset and with room for the blur,
//!   so geometry off the bitmap still casts a shadow onto it.
//! - Paths arrive in device coordinates (the canvas transform was applied as
//!   they were built); strokes are traced in user space and transformed, as
//!   HTML specifies, so a non-uniform transform shears the pen.
//!
//! Also here: the process's system fonts ([`fonts`], shared with page
//! rendering), canvas text layout ([`text`]) and image decode/encode
//! ([`codec`]).

pub mod codec;
mod composite;
pub mod fonts;
pub mod image_decoder;
mod guard;
pub mod text;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

pub use kurbo;
pub use peniko;
pub use vello_cpu::Pixmap;

use guard::guard_path;
use kurbo::{Affine, BezPath, PathEl, Point, Rect, Shape, Stroke, Vec2};
use peniko::color::{AlphaColor, Srgb};
use peniko::{Extend, Fill, ImageQuality, ImageSampler};
use vello_cpu::{
    Image, ImageSource, PaintType, RasterizerSettings, RenderContext, RenderSettings, Resources, TargetInit,
};

/// A straight-alpha sRGB color.
pub type Color = AlphaColor<Srgb>;

/// Largest bitmap side, in pixels.
pub const MAX_SIDE: u32 = 16_384;
/// Largest bitmap area, in pixels (128 MiB of RGBA).
pub const MAX_AREA: u64 = 1 << 25;
/// Draws batched before the batch is rasterized, which bounds the memory a
/// page's unread drawing holds.
const MAX_BATCH: u32 = 4096;

/// `globalCompositeOperation`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Composite {
    SourceOver,
    SourceIn,
    SourceOut,
    SourceAtop,
    DestinationOver,
    DestinationIn,
    DestinationOut,
    DestinationAtop,
    Lighter,
    Copy,
    Xor,
    Multiply,
    Screen,
    Overlay,
    Darken,
    Lighten,
    ColorDodge,
    ColorBurn,
    HardLight,
    SoftLight,
    Difference,
    Exclusion,
    Hue,
    Saturation,
    Color,
    Luminosity,
    /// Porter-Duff clear (Fa = 0, Fb = 0).
    Clear,
}

impl Composite {
    pub const ALL: [(&'static str, Composite); 27] = [
        ("source-over", Composite::SourceOver),
        ("source-in", Composite::SourceIn),
        ("source-out", Composite::SourceOut),
        ("source-atop", Composite::SourceAtop),
        ("destination-over", Composite::DestinationOver),
        ("destination-in", Composite::DestinationIn),
        ("destination-out", Composite::DestinationOut),
        ("destination-atop", Composite::DestinationAtop),
        ("lighter", Composite::Lighter),
        ("copy", Composite::Copy),
        ("xor", Composite::Xor),
        ("multiply", Composite::Multiply),
        ("screen", Composite::Screen),
        ("overlay", Composite::Overlay),
        ("darken", Composite::Darken),
        ("lighten", Composite::Lighten),
        ("color-dodge", Composite::ColorDodge),
        ("color-burn", Composite::ColorBurn),
        ("hard-light", Composite::HardLight),
        ("soft-light", Composite::SoftLight),
        ("difference", Composite::Difference),
        ("exclusion", Composite::Exclusion),
        ("hue", Composite::Hue),
        ("saturation", Composite::Saturation),
        ("color", Composite::Color),
        ("luminosity", Composite::Luminosity),
        ("clear", Composite::Clear),
    ];

    pub fn from_name(name: &str) -> Option<Composite> {
        Self::ALL.iter().find(|(n, _)| *n == name).map(|(_, c)| *c)
    }

    pub fn name(self) -> &'static str {
        Self::ALL.iter().find(|(_, c)| *c == self).map_or("source-over", |(n, _)| n)
    }
}

/// A canvas gradient's geometry (user space).
#[derive(Clone, Debug, PartialEq)]
pub enum GradientKind {
    Linear {
        start: Point,
        end: Point,
    },
    Radial {
        start: Point,
        start_radius: f64,
        end: Point,
        end_radius: f64,
    },
    /// `createConicGradient(angle, x, y)`: clockwise from `angle` (radians
    /// from the positive x axis).
    Conic {
        center: Point,
        angle: f64,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Gradient {
    pub kind: GradientKind,
    /// In the order `addColorStop` added them (a stable sort by offset is
    /// applied when painting).
    pub stops: Vec<(f32, Color)>,
    /// Interpolate in Oklab instead of sRGB: Chrome does so when a stop's color is not a legacy
    /// sRGB syntax (`color-mix()`, relative colors, `color()`, `lab()`, ...), WPT 2d.gradient.colormix.
    pub oklab: bool,
}

#[derive(Clone, Debug)]
pub struct Pattern {
    pub image: Bitmap,
    pub repeat_x: bool,
    pub repeat_y: bool,
    /// `CanvasPattern.setTransform`.
    pub transform: Affine,
}

/// A fill or stroke style.
#[derive(Clone, Debug)]
pub enum Paint {
    Color(Color),
    Gradient(Arc<Gradient>),
    Pattern(Arc<Pattern>),
}

/// One `clip()` of the clipping region: the path in device coordinates.
#[derive(Clone, Debug, PartialEq)]
pub struct ClipPath {
    pub path: BezPath,
    pub evenodd: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shadow {
    pub color: Color,
    /// `shadowBlur` (the Gaussian's standard deviation is half of it).
    pub blur: f64,
    /// Device pixels: shadow offsets ignore the transform.
    pub offset: Vec2,
}

/// The drawing state a draw uses.
#[derive(Clone, Debug)]
pub struct DrawState<'a> {
    pub transform: Affine,
    pub alpha: f32,
    pub composite: Composite,
    /// Intersected; empty means no clipping.
    pub clip: &'a [ClipPath],
    pub shadow: Option<Shadow>,
    /// `imageSmoothingEnabled` (bilinear) or nearest neighbour.
    pub smoothing: bool,
    /// pyro: `filter` (none: `None`).
    pub filter: Option<&'a CanvasFilter>,
}

/// pyro: a canvas `filter` other than `none`: the value as given (CSS filter functions and
/// `url(#id)` references) and the markup of the document's elements it references by id (SVG
/// `<filter>`s), which the registered [`FilterRenderer`] applies to each draw's output.
#[derive(Clone, Debug, PartialEq)]
pub struct CanvasFilter {
    pub page: pyro_raster::PageRasters,
    pub value: String,
    pub defs: String,
}

/// Applies a canvas filter to a draw's output (premultiplied, the canvas's size), or `None`
/// when it cannot (the output is then drawn unfiltered).
pub type FilterRenderer = fn(&Pixmap, &CanvasFilter) -> Option<Pixmap>;

static FILTER_RENDERER: std::sync::OnceLock<FilterRenderer> = std::sync::OnceLock::new();

/// Registers the renderer of canvas filters (`pyro-render` registers one).
pub fn set_filter_renderer(renderer: FilterRenderer) {
    let _ = FILTER_RENDERER.set(renderer);
}

impl DrawState<'_> {
    fn visible_shadow(&self) -> Option<Shadow> {
        self.shadow.filter(|s| s.color.components[3] > 0.0 && (s.blur > 0.0 || s.offset.x != 0.0 || s.offset.y != 0.0))
    }
}

/// Image encodings of `toDataURL`/`toBlob`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Encoding {
    Png,
    /// Quality in 0..=1.
    Jpeg(f32),
    /// Lossless (the encoder available).
    Webp,
}

struct Batch {
    ctx: RenderContext,
    resources: Resources,
    ops: u32,
}

/// One canvas's bitmap.
pub struct Surface {
    width: u32,
    height: u32,
    /// Premultiplied RGBA; `None` until something is drawn. Shared with
    /// snapshots (`drawImage` of this canvas, patterns, `ImageBitmap`s) and
    /// copied on the next write while one is alive.
    pixmap: Option<Bitmap>,
    batch: Option<Box<Batch>>,
    /// Bumped by every change of the pixels.
    generation: u64,
    /// The straight-alpha RGBA of [`Surface::straight_rgba`], for the
    /// generation it was made at.
    straight: Option<(u64, Arc<Vec<u8>>)>,
}

/// Bytes of the process's live [`Bitmap`]s: surfaces' bitmaps and every
/// snapshot, `ImageBitmap`, pattern and decoded image that outlives them.
static ALLOCATED: AtomicUsize = AtomicUsize::new(0);
/// Most bitmap bytes the process holds: past it, a draw that needs a new
/// bitmap draws nothing and an image that needs one does not decode (as a
/// browser's failed allocation).
const MAX_TOTAL: usize = 2 << 30;

/// [`ALLOCATED`] bytes, given back when the last handle to the pixels they
/// count goes.
#[derive(Debug)]
struct Charge(usize);

impl Charge {
    fn new(bytes: usize) -> Option<Charge> {
        let total = ALLOCATED.fetch_add(bytes, Ordering::Relaxed) + bytes;
        if total > MAX_TOTAL {
            ALLOCATED.fetch_sub(bytes, Ordering::Relaxed);
            return None;
        }
        Some(Charge(bytes))
    }
}

impl Drop for Charge {
    fn drop(&mut self) {
        ALLOCATED.fetch_sub(self.0, Ordering::Relaxed);
    }
}

/// Premultiplied pixels counted toward the process's bitmap budget
/// ([`MAX_TOTAL`]) for as long as any clone of the handle lives, whichever
/// surface, snapshot or image made it.
#[derive(Clone)]
pub struct Bitmap {
    page_charge: Option<Arc<pyro_raster::Reservation>>,
    pixmap: Arc<Pixmap>,
    _charge: Arc<Charge>,
}

impl Bitmap {
    /// `pixmap`, counted; `None` when the process's budget has no room.
    pub fn new(pixmap: Pixmap) -> Option<Bitmap> {
        let charge = Charge::new(pixmap.data_as_u8_slice().len())?;
        Some(Bitmap { pixmap: Arc::new(pixmap), _charge: Arc::new(charge), page_charge: None })
    }

    /// The pixels as the rasterizer takes them.
    pub fn pixmap(&self) -> &Arc<Pixmap> {
        &self.pixmap
    }

    /// Bytes this bitmap counts.
    pub fn byte_len(&self) -> usize {
        self._charge.0
    }
}

impl std::ops::Deref for Bitmap {
    type Target = Pixmap;
    fn deref(&self) -> &Pixmap {
        &self.pixmap
    }
}

impl std::fmt::Debug for Bitmap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Bitmap({}x{})", self.pixmap.width(), self.pixmap.height())
    }
}

/// Bitmap bytes the process holds now ([`Bitmap`]).
pub fn allocated_bytes() -> usize {
    ALLOCATED.load(Ordering::Relaxed)
}

fn settings(width: u16, height: u16) -> RenderContext {
    // One thread: a page's canvas work stays on its thread, like its JS.
    let settings = RenderSettings { num_threads: 0, ..RenderSettings::default() };
    RenderContext::new_with(width, height, settings)
}

/// Whether a `width` by `height` bitmap can be allocated.
pub fn valid_size(width: u32, height: u32) -> bool {
    width > 0
        && height > 0
        && width <= MAX_SIDE
        && height <= MAX_SIDE
        && u64::from(width) * u64::from(height) <= MAX_AREA
}

impl Surface {
    pub fn new(width: u32, height: u32) -> Surface {
        Surface { width, height, pixmap: None, batch: None, generation: 0, straight: None }
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    /// Whether the bitmap exists (something was drawn or written).
    pub fn is_allocated(&self) -> bool {
        self.pixmap.is_some() || self.batch.as_ref().is_some_and(|b| b.ops > 0)
    }

    /// Changes whenever the pixels may have.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// A new size (or the same one): the bitmap is cleared and released.
    pub fn resize(&mut self, width: u32, height: u32) {
        self.width = width;
        self.height = height;
        self.pixmap = None;
        self.batch = None;
        self.straight = None;
        self.generation += 1;
    }

    fn drawable(&self) -> Option<(u16, u16)> {
        valid_size(self.width, self.height).then_some((self.width as u16, self.height as u16))
    }

    /// [`Surface::drawable`], once the bitmap exists (allocated on the
    /// first draw, when the process's bitmap budget has room for it).
    fn reserve(&mut self) -> Option<(u16, u16)> {
        let size = self.drawable()?;
        if self.pixmap.is_none() {
            self.pixmap_mut()?;
        }
        Some(size)
    }

    /// The bitmap, allocated, for writing. One a snapshot still shares is
    /// copied first; the copy is counted, the snapshot keeps its own count.
    fn pixmap_mut(&mut self) -> Option<&mut Pixmap> {
        let (w, h) = self.drawable()?;
        let unique = self.pixmap.as_mut().is_some_and(|b| Arc::get_mut(&mut b.pixmap).is_some());
        if !unique {
            let pixmap = match &self.pixmap {
                Some(shared) => Pixmap::clone(&shared.pixmap),
                None => Pixmap::new(w, h),
            };
            self.pixmap = Some(Bitmap::new(pixmap)?);
        }
        Arc::get_mut(&mut self.pixmap.as_mut()?.pixmap)
    }

    /// Rasterizes batched draws onto the bitmap.
    pub fn flush(&mut self) {
        let Some(mut batch) = self.batch.take() else { return };
        if batch.ops > 0 {
            if let Some(pixmap) = self.pixmap_mut() {
                batch.ctx.flush();
                let settings = RasterizerSettings { target_init: TargetInit::SrcOver, ..RasterizerSettings::default() };
                batch.ctx.render_with(pixmap, &mut batch.resources, settings);
            }
            batch.ctx.reset();
            batch.ops = 0;
        }
        self.batch = Some(batch);
    }

    fn changed(&mut self) {
        self.generation += 1;
        // The straight copy is stale: release it now rather than at the next
        // snapshot.
        self.straight = None;
    }

    /// The bitmap as it is now (premultiplied), `None` while nothing was
    /// drawn (transparent black).
    pub fn snapshot(&mut self) -> Option<Bitmap> {
        self.flush();
        self.pixmap.clone()
    }

    /// Straight-alpha RGBA rows of the whole bitmap, `None` while nothing
    /// was drawn. Made once per change of the pixels.
    pub fn straight_rgba(&mut self) -> Option<Arc<Vec<u8>>> {
        self.flush();
        let pixmap = self.pixmap.as_ref()?;
        if let Some((generation, data)) = &self.straight {
            if *generation == self.generation {
                return Some(data.clone());
            }
        }
        let mut data = pixmap.data_as_u8_slice().to_vec();
        unpremultiply(&mut data);
        let data = Arc::new(data);
        self.straight = Some((self.generation, data.clone()));
        Some(data)
    }

    /// Fills `path` (device coordinates).
    pub fn fill(&mut self, path: &BezPath, evenodd: bool, paint: &Paint, state: &DrawState<'_>) {
        let path = fine_flatten(guard_path(path, self.width, self.height), self.width, self.height);
        let rule = if evenodd { Fill::EvenOdd } else { Fill::NonZero };
        self.draw(paint, state, Affine::IDENTITY, &mut |ctx, _| {
            ctx.set_fill_rule(rule);
            ctx.fill_path(&path);
        });
    }

    /// Strokes `path` (device coordinates) with `stroke` in the user space
    /// of `state.transform`. A transform that cannot be inverted draws
    /// nothing.
    pub fn stroke(&mut self, path: &BezPath, stroke: &Stroke, paint: &Paint, state: &DrawState<'_>) {
        let t = state.transform;
        if !invertible(t) {
            return;
        }
        let user = t.inverse() * guard_path(path, self.width, self.height);
        let mut stroke = stroke.clone();
        // A pen wider than the canvas many times over covers what the
        // canvas shows; the stroker's work grows with the width. pyro: the
        // pen is narrowed no further than twice the extent of the path and
        // the canvas together (user space; up to 16 times the plain limit):
        // every canvas point then lies within half a pen of every path
        // point, so the body, caps and joins (miters scale about their
        // corner) cover the canvas as the wider pen does (WPT
        // 2d.line.miter.obtuse: a 1600 px miter corner 800 px off a 100 px
        // canvas).
        let scale = stretch(t);
        let mut limit = 4.0 * (f64::from(self.width) + f64::from(self.height)) / scale.max(1e-9);
        if stroke.width > limit {
            let inverse = t.inverse();
            let canvas = Rect::new(0.0, 0.0, f64::from(self.width), f64::from(self.height));
            let corners = [
                canvas.origin(),
                Point::new(canvas.x1, 0.0),
                Point::new(0.0, canvas.y1),
                Point::new(canvas.x1, canvas.y1),
            ];
            let reach = corners.iter().fold(user.bounding_box(), |r, c| r.union_pt(inverse * *c));
            let extent = 2.0 * reach.width().hypot(reach.height());
            if extent.is_finite() {
                limit = limit.max(extent.min(16.0 * limit));
            }
        }
        if stroke.width > limit {
            stroke.width = limit;
        }
        // Too many dashes to trace (a tiny pattern along a long path) draw
        // the path solid, as Skia does past its dash count limit.
        if !stroke.dash_pattern.is_empty() {
            let period: f64 = stroke.dash_pattern.iter().sum();
            if period.is_nan() || period <= 0.0 || control_length(&user) / period > MAX_DASHES {
                stroke.dash_pattern.clear();
            }
        }
        let union = if stroke.dash_pattern.is_empty() { overlapping_polyline_stroke(&user, &stroke) } else { None };
        self.draw(paint, state, t, &mut |ctx, _| {
            if let Some(union) = &union {
                ctx.set_fill_rule(Fill::NonZero);
                ctx.fill_path(union);
            } else {
                ctx.set_stroke(stroke.clone());
                ctx.stroke_path(&user);
            }
        });
    }

    /// `drawImage`: the `src` rectangle of `image` into the `dst` rectangle
    /// (user space). `src` is clipped to the image, `dst` with it.
    pub fn draw_image(&mut self, image: &Bitmap, src: Rect, dst: Rect, state: &DrawState<'_>) {
        let size = (f64::from(image.width()), f64::from(image.height()));
        let Some((dst, to_dst)) = image_placement(size, src, dst) else { return };
        let pattern = Pattern { image: image.clone(), repeat_x: false, repeat_y: false, transform: to_dst };
        let paint = Paint::Pattern(Arc::new(pattern));
        let t = state.transform;
        if !invertible(t) {
            return;
        }
        // The destination as a device path, guarded like any other.
        let path = guard_path(&(t * dst.to_path(0.1)), self.width, self.height);
        self.draw(&paint, state, Affine::IDENTITY, &mut |ctx, _| ctx.fill_path(&path));
    }

    /// `drawImage` of a `width` by `height` source without a bitmap (a canvas
    /// with no context, or one not drawn on yet): transparent black pixels,
    /// which change the bitmap only under operators that a transparent source
    /// does not leave alone (`copy`, `source-in`, ...).
    pub fn draw_transparent_image(&mut self, width: u32, height: u32, src: Rect, dst: Rect, state: &DrawState<'_>) {
        if composite::bounded(state.composite) && state.filter.is_none() {
            return;
        }
        let Some((dst, _)) = image_placement((f64::from(width), f64::from(height)), src, dst) else { return };
        let t = state.transform;
        if !invertible(t) {
            return;
        }
        let path = guard_path(&(t * dst.to_path(0.1)), self.width, self.height);
        let paint = Paint::Color(Color::from_rgba8(0, 0, 0, 0));
        self.draw(&paint, state, Affine::IDENTITY, &mut |ctx, _| ctx.fill_path(&path));
    }

    /// Draws a laid out text at `origin` (user space: the alphabetic baseline
    /// of the first glyph), squeezed horizontally by `x_scale`.
    pub fn text(
        &mut self,
        layout: &text::TextLayout,
        origin: Point,
        x_scale: f64,
        stroke: Option<&Stroke>,
        paint: &Paint,
        state: &DrawState<'_>,
    ) {
        let t = state.transform;
        if !invertible(t) || layout.runs.is_empty() {
            return;
        }
        let g = t * Affine::translate(origin.to_vec2()) * Affine::scale_non_uniform(x_scale, 1.0);
        // Glyphs this large (device px) or this far away would take the
        // rasterizer long to outline and show nothing a canvas can hold.
        let device_size = stretch(g) * f64::from(layout.runs[0].size);
        let far = 1e7 + 64.0 * f64::from(self.width.max(self.height));
        let [.., e, f] = g.as_coeffs();
        if device_size.is_nan() || device_size > 1e5 || !(e.abs() <= far && f.abs() <= far) {
            return;
        }
        self.draw(paint, state, g, &mut |ctx, resources| {
            if let Some(stroke) = stroke {
                ctx.set_stroke(stroke.clone());
            }
            for run in &layout.runs {
                let glyphs = run.glyphs.iter().copied();
                let mut builder = ctx.glyph_run(resources, &run.font).font_size(run.size).hint(false);
                if let Some(skew) = run.skew {
                    builder = builder.glyph_transform(Affine::skew(f64::from(skew).to_radians().tan(), 0.0));
                }
                let builder = builder.normalized_coords(bytemuck::cast_slice(&run.coords));
                let _ = if stroke.is_some() { builder.stroke_glyphs(glyphs) } else { builder.fill_glyphs(glyphs) };
            }
        });
    }

    /// `clearRect`: the rectangle (user space), within the clipping region,
    /// becomes transparent black.
    pub fn clear_rect(&mut self, rect: Rect, transform: Affine, clip: &[ClipPath]) {
        let Some((w, h)) = self.drawable() else { return };
        if self.pixmap.is_none() && !self.is_allocated() {
            return;
        }
        let rect = rect.abs();
        if rect.width() == 0.0 || rect.height() == 0.0 || !invertible(transform) {
            return;
        }
        let device = transform.transform_rect_bbox(rect);
        let whole = Rect::new(0.0, 0.0, f64::from(w), f64::from(h));
        let axis_aligned = transform.as_coeffs()[1] == 0.0 && transform.as_coeffs()[2] == 0.0;
        if clip.is_empty() && axis_aligned && device.contains_rect(whole) {
            // The whole bitmap: what was batched is dropped with it.
            if let Some(batch) = self.batch.as_mut() {
                batch.ctx.reset();
                batch.ops = 0;
            }
            match self.pixmap.as_mut().and_then(|b| Arc::get_mut(&mut b.pixmap)) {
                Some(pixmap) => pixmap.data_as_u8_slice_mut().fill(0),
                // Shared with a snapshot: no bitmap reads as transparent
                // black, and the next draw allocates a new one.
                None => self.pixmap = None,
            }
            self.changed();
            return;
        }
        self.flush();
        let coverage = coverage(w, h, &mut |ctx| {
            ctx.set_transform(transform);
            ctx.fill_rect(&rect);
        });
        let mask = self.clip_mask(clip);
        let Some(pixmap) = self.pixmap_mut() else { return };
        composite::clear(pixmap.data_as_u8_slice_mut(), &coverage, mask.as_deref());
        self.changed();
    }

    /// Straight RGBA of the `w` by `h` rectangle at (`x`, `y`) (any of it
    /// outside the bitmap is transparent black).
    pub fn get_image_data(&mut self, x: i64, y: i64, w: u32, h: u32) -> Vec<u8> {
        let mut out = vec![0u8; w as usize * h as usize * 4];
        self.flush();
        let Some(pixmap) = self.pixmap.as_ref() else { return out };
        let (pw, ph) = (i64::from(pixmap.width()), i64::from(pixmap.height()));
        let data = pixmap.data_as_u8_slice();
        for row in 0..i64::from(h) {
            let sy = y + row;
            if sy < 0 || sy >= ph {
                continue;
            }
            let x0 = x.max(0);
            let x1 = (x + i64::from(w)).min(pw);
            if x0 >= x1 {
                continue;
            }
            let src = &data[((sy * pw + x0) * 4) as usize..((sy * pw + x1) * 4) as usize];
            let start = ((row * i64::from(w) + (x0 - x)) * 4) as usize;
            let dst = &mut out[start..start + src.len()];
            dst.copy_from_slice(src);
            unpremultiply(dst);
        }
        out
    }

    /// `putImageData`: straight RGBA `data` (`width` by `height`) written at
    /// (`dx`, `dy`), only its `dirty` rectangle (in `data`'s pixels, already
    /// clamped to it). No compositing, transform or clipping.
    pub fn put_image_data(
        &mut self,
        data: &[u8],
        width: u32,
        height: u32,
        dx: i64,
        dy: i64,
        dirty: (u32, u32, u32, u32),
    ) {
        if data.len() < width as usize * height as usize * 4 {
            return;
        }
        let (bx, by, bw, bh) = dirty;
        if bw == 0 || bh == 0 {
            return;
        }
        let Some((pw, ph)) = self.drawable() else { return };
        // Nothing of it lands on the bitmap: nothing to allocate.
        let (x0, y0) = ((dx + i64::from(bx)).max(0), (dy + i64::from(by)).max(0));
        let (x1, y1) = (
            (dx + i64::from(bx) + i64::from(bw)).min(i64::from(pw)),
            (dy + i64::from(by) + i64::from(bh)).min(i64::from(ph)),
        );
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        self.flush();
        let Some(pixmap) = self.pixmap_mut() else { return };
        let pw = i64::from(pw);
        let out = pixmap.data_as_u8_slice_mut();
        for y in y0..y1 {
            let sy = y - dy;
            let src_start = ((sy * i64::from(width) + (x0 - dx)) * 4) as usize;
            let src = &data[src_start..src_start + ((x1 - x0) * 4) as usize];
            let dst_start = ((y * pw + x0) * 4) as usize;
            let dst = &mut out[dst_start..dst_start + src.len()];
            dst.copy_from_slice(src);
            premultiply(dst);
        }
        self.changed();
    }

    /// The bitmap in `encoding`; `None` when it has no pixels to encode
    /// (a zero-sized or too large canvas) or the encoder failed.
    pub fn encode(&mut self, encoding: Encoding) -> Option<Vec<u8>> {
        let (w, h) = self.drawable()?;
        let rgba = match self.straight_rgba() {
            Some(data) => data,
            None => Arc::new(vec![0; usize::from(w) * usize::from(h) * 4]),
        };
        codec::encode(&rgba, u32::from(w), u32::from(h), encoding)
    }

    /// One draw: `draw` issues the geometry with the paint set, in the
    /// coordinate system `geometry` (the scene transform it should use).
    fn draw(
        &mut self,
        paint: &Paint,
        state: &DrawState<'_>,
        geometry: Affine,
        draw: &mut dyn FnMut(&mut RenderContext, &mut Resources),
    ) {
        if !state.alpha.is_finite() || state.alpha <= 0.0 && state.composite == Composite::SourceOver {
            return;
        }
        let Some((w, h)) = self.reserve() else { return };
        let shadow = state.visible_shadow();
        let filter = state.filter.filter(|_| FILTER_RENDERER.get().is_some());
        if state.composite == Composite::SourceOver && shadow.is_none() && filter.is_none() {
            let full = self.batch.as_ref().is_some_and(|b| b.ops >= MAX_BATCH);
            if full {
                self.flush();
            }
            let batch = self
                .batch
                .get_or_insert_with(|| Box::new(Batch { ctx: settings(w, h), resources: Resources::new(), ops: 0 }));
            emit(&mut batch.ctx, &mut batch.resources, paint, state, geometry, state.alpha, state.clip, (w, h), draw);
            batch.ops += 1;
            self.changed();
            return;
        }
        self.flush();
        let mut ctx = settings(w, h);
        let mut resources = Resources::new();
        emit(&mut ctx, &mut resources, paint, state, geometry, 1.0, &[], (w, h), draw);
        let mut source = Pixmap::new(w, h);
        ctx.flush();
        ctx.render(&mut source, &mut resources);
        // pyro: the filter applies to the draw's output before its shadow and compositing (HTML
        // canvas drawing model).
        if let (Some(filter), Some(render)) = (filter, FILTER_RENDERER.get()) {
            if let Some(filtered) = render(&source, filter).filter(|p| p.width() == w && p.height() == h) {
                source = filtered;
            }
        }
        // pyro: the shadow is cast by the whole drawing, also its parts off the bitmap that its
        // offset or blur bring onto it, so (without a filter, which needs the drawing at the
        // bitmap's size) its source is rendered apart, already offset, with room for the blur.
        let shadow_source = match shadow {
            Some(shadow) if filter.is_none() => Some(shadow_source(paint, state, geometry, (w, h), &shadow, draw)),
            _ => None,
        };
        let mask = self.clip_mask(state.clip);
        let op = state.composite;
        let alpha = state.alpha.clamp(0.0, 1.0);
        let Some(pixmap) = self.pixmap_mut() else { return };
        if let Some(shadow) = shadow {
            let shadow = match shadow_source {
                Some(ShadowSource { pixmap: s, pad }) => composite::shadow_cropped(
                    s.data_as_u8_slice(),
                    usize::from(s.width()),
                    usize::from(s.height()),
                    pad,
                    usize::from(w),
                    usize::from(h),
                    &shadow,
                ),
                None => composite::shadow(source.data_as_u8_slice(), usize::from(w), usize::from(h), &shadow),
            };
            composite::apply(pixmap.data_as_u8_slice_mut(), &shadow, alpha, op, mask.as_deref());
        }
        composite::apply(pixmap.data_as_u8_slice_mut(), source.data_as_u8_slice(), alpha, op, mask.as_deref());
        self.changed();
    }

    /// The clipping region's coverage per pixel, `None` without clipping.
    fn clip_mask(&self, clip: &[ClipPath]) -> Option<Vec<u8>> {
        if clip.is_empty() {
            return None;
        }
        let (w, h) = self.drawable()?;
        let paths: Vec<ClipPath> = clip
            .iter()
            .map(|c| ClipPath { path: guard_path(&c.path, self.width, self.height), evenodd: c.evenodd })
            .collect();
        Some(coverage(w, h, &mut |ctx| {
            for c in &paths {
                ctx.set_fill_rule(if c.evenodd { Fill::EvenOdd } else { Fill::NonZero });
                ctx.push_clip_path(&c.path);
            }
            ctx.fill_rect(&Rect::new(0.0, 0.0, f64::from(w), f64::from(h)));
            for _ in &paths {
                ctx.pop_clip();
            }
        }))
    }
}

/// A draw's shadow source: the drawing offset by the shadow's offset, on a
/// bitmap `pad` pixels larger than the canvas on each side (the reach of the
/// blur), so geometry off the canvas still casts its shadow onto it.
struct ShadowSource {
    pixmap: Pixmap,
    pad: usize,
}

fn shadow_source(
    paint: &Paint,
    state: &DrawState<'_>,
    geometry: Affine,
    (w, h): (u16, u16),
    shadow: &Shadow,
    draw: &mut dyn FnMut(&mut RenderContext, &mut Resources),
) -> ShadowSource {
    let side = u32::from(w.max(h));
    // Room for the blur (three box passes of `composite::blur_box` each),
    // bounded so the bitmap stays a valid canvas size.
    let reach = (composite::blur_box(shadow.blur / 2.0, usize::try_from(side).unwrap_or(usize::MAX)) * 3 / 2 + 1) as u32;
    let room = (MAX_SIDE - side) / 2;
    let mut pad = reach.min(room);
    while pad > 0 && !valid_size(u32::from(w) + 2 * pad, u32::from(h) + 2 * pad) {
        pad /= 2;
    }
    let (pw, ph) = ((u32::from(w) + 2 * pad) as u16, (u32::from(h) + 2 * pad) as u16);
    let mut pixmap = Pixmap::new(pw, ph);
    // Geometry within range of the canvas (see `guard`) cannot be offset onto
    // it from farther than this.
    let far = 2.0 * (1e6 + 64.0 * f64::from(side));
    if !(shadow.offset.x.abs() <= far && shadow.offset.y.abs() <= far) {
        return ShadowSource { pixmap, pad: pad as usize };
    }
    let shift = Affine::translate(shadow.offset + Vec2::new(f64::from(pad), f64::from(pad)));
    let shifted = DrawState { transform: shift * state.transform, ..state.clone() };
    let mut ctx = settings(pw, ph);
    let mut resources = Resources::new();
    emit(&mut ctx, &mut resources, paint, &shifted, shift * geometry, 1.0, &[], (pw, ph), draw);
    ctx.flush();
    ctx.render(&mut pixmap, &mut resources);
    ShadowSource { pixmap, pad: pad as usize }
}

/// The coverage (alpha) of what `draw` fills, per pixel.
fn coverage(w: u16, h: u16, draw: &mut dyn FnMut(&mut RenderContext)) -> Vec<u8> {
    let mut ctx = settings(w, h);
    ctx.set_paint(Color::WHITE);
    draw(&mut ctx);
    let mut pixmap = Pixmap::new(w, h);
    let mut resources = Resources::new();
    ctx.flush();
    ctx.render(&mut pixmap, &mut resources);
    pixmap.data_as_u8_slice().chunks_exact(4).map(|p| p[3]).collect()
}

/// Where `drawImage` puts an image of `size`: `src` clipped to the image and
/// `dst` with it, and the image-to-user transform; `None` when nothing is drawn.
fn image_placement(size: (f64, f64), src: Rect, dst: Rect) -> Option<(Rect, Affine)> {
    let bounds = Rect::new(0.0, 0.0, size.0, size.1);
    let (src, dst) = (src.abs(), dst.abs());
    if src.width() == 0.0 || src.height() == 0.0 || dst.width() == 0.0 || dst.height() == 0.0 {
        return None;
    }
    let clipped = src.intersect(bounds);
    if clipped.width() <= 0.0 || clipped.height() <= 0.0 {
        return None;
    }
    let (sx, sy) = (dst.width() / src.width(), dst.height() / src.height());
    let dst = Rect::new(
        dst.x0 + (clipped.x0 - src.x0) * sx,
        dst.y0 + (clipped.y0 - src.y0) * sy,
        dst.x0 + (clipped.x1 - src.x0) * sx,
        dst.y0 + (clipped.y1 - src.y0) * sy,
    );
    let to_dst = Affine::translate(dst.origin().to_vec2())
        * Affine::scale_non_uniform(sx, sy)
        * Affine::translate(-clipped.origin().to_vec2());
    Some((dst, to_dst))
}

/// How much `t` stretches lengths at most (a bound on its largest singular
/// value).
fn stretch(t: Affine) -> f64 {
    let [a, b, c, d, _, _] = t.as_coeffs();
    (a.abs() + c.abs()).max(b.abs() + d.abs())
}

/// The area a pen of `stroke.width` covers along a polyline `path` (lines only) whose joins have
/// an inner side the pen overreaches: a segment shorter than `radius * tan(turn / 2)` before a
/// join. kurbo's stroker offsets each side as one outline and, with a bevel or miter join, closes
/// the inner side by a chord between the two offset points, which sweeps a wedge the pen never
/// covered (WPT 2d.line.cross: a 200 px wide bevel-joined stroke filled a corner of the canvas).
/// Skia joins the inner side through the vertex. Here the stroke is the union of one rectangle per
/// segment and the outer wedge of each join (bevel triangle, miter quadrilateral within the
/// limit, a disc for round), all oriented alike so that a nonzero fill unites them. `None` for a
/// path with curves, a subpath without length, or when no join overreaches (kurbo's outline is
/// exact then and stays the one used).
fn overlapping_polyline_stroke(path: &BezPath, stroke: &Stroke) -> Option<BezPath> {
    use kurbo::{Circle, Join, Cap};
    let radius = stroke.width / 2.0;
    if !(radius.is_finite() && radius > 0.0) {
        return None;
    }
    // Subpaths as point lists without repeated points, and whether each was closed.
    let mut subpaths: Vec<(Vec<Point>, bool)> = Vec::new();
    for el in path.elements() {
        match *el {
            PathEl::MoveTo(p) => subpaths.push((vec![p], false)),
            PathEl::LineTo(p) => {
                let (points, _) = subpaths.last_mut()?;
                if points.last() != Some(&p) {
                    points.push(p);
                }
            }
            PathEl::ClosePath => {
                let (points, closed) = subpaths.last_mut()?;
                *closed = true;
                if points.len() > 1 && points.first() == points.last() {
                    points.pop();
                }
            }
            _ => return None,
        }
    }
    let direction = |a: Point, b: Point| (b - a) / (b - a).hypot();
    // A closed subpath's segments, in order, wrapping around.
    let segments = |points: &[Point], closed: bool| -> usize { if closed { points.len() } else { points.len() - 1 } };
    let mut overreaches = false;
    for (points, closed) in &subpaths {
        if points.len() < 2 || (*closed && points.len() < 3) {
            return None;
        }
        let n = points.len();
        let count = segments(points, *closed);
        for i in 0..count {
            let (a, b) = (points[i], points[(i + 1) % n]);
            let next = if *closed || i + 1 < count { Some((b, points[(i + 2) % n])) } else { None };
            let Some((b, c)) = next else { continue };
            let (d1, d2) = (direction(a, b), direction(b, c));
            let phi = d1.dot(d2).clamp(-1.0, 1.0).acos();
            let reach = radius * (phi / 2.0).tan();
            if !(reach <= (b - a).hypot().min((c - b).hypot())) {
                overreaches = true;
            }
        }
    }
    if !overreaches {
        return None;
    }
    let mut out = BezPath::new();
    let mut add = |mut shape: BezPath| {
        if shape.area() < 0.0 {
            shape = shape.reverse_subpaths();
        }
        out.extend(shape);
    };
    let polygon = |points: &[Point]| {
        let mut bez = BezPath::new();
        bez.move_to(points[0]);
        for p in &points[1..] {
            bez.line_to(*p);
        }
        bez.close_path();
        bez
    };
    let disc = |center: Point| Circle::new(center, radius).to_path(0.01);
    let normal = |d: Vec2| Vec2::new(-d.y, d.x) * radius;
    for (points, closed) in &subpaths {
        let n = points.len();
        let count = segments(points, *closed);
        for i in 0..count {
            let (a, b) = (points[i], points[(i + 1) % n]);
            let d = direction(a, b);
            let nm = normal(d);
            // Open ends take their cap: a square one extends the rectangle, a round one is a disc.
            let (mut from, mut to) = (a, b);
            if !*closed {
                if i == 0 {
                    match stroke.start_cap {
                        Cap::Square => from = a - d * radius,
                        Cap::Round => add(disc(a)),
                        Cap::Butt => {}
                    }
                }
                if i + 1 == count {
                    match stroke.end_cap {
                        Cap::Square => to = b + d * radius,
                        Cap::Round => add(disc(b)),
                        Cap::Butt => {}
                    }
                }
            }
            add(polygon(&[from + nm, to + nm, to - nm, from - nm]));
            let next = if *closed || i + 1 < count { Some(points[(i + 2) % n]) } else { None };
            let Some(c) = next else { continue };
            let d2 = direction(b, c);
            let (cross, dot) = (d.cross(d2), d.dot(d2));
            if cross.abs() < 1e-12 && dot > 0.0 {
                continue;
            }
            match stroke.join {
                Join::Round => add(disc(b)),
                Join::Bevel | Join::Miter => {
                    // The outer side is the right of a left turn, the left of a right turn.
                    let side = if cross > 0.0 { -1.0 } else { 1.0 };
                    let (na, nb) = (normal(d) * side, normal(d2) * side);
                    let miter_ok = stroke.join == Join::Miter && (1.0 + dot) / 2.0 >= 1.0 / (stroke.miter_limit * stroke.miter_limit);
                    if miter_ok && dot > -1.0 {
                        let tip = b + (na + nb) / (1.0 + dot);
                        add(polygon(&[b, b + na, tip, b + nb]));
                    } else {
                        add(polygon(&[b, b + na, b + nb]));
                    }
                }
            }
        }
    }
    Some(out)
}

/// `path` with its curves flattened to a hundredth of a pixel. vello_cpu flattens at a quarter of
/// a pixel, which cuts a corner of a curved edge by up to that much: a pixel that lies fully inside
/// a large ellipse came out a twentieth uncovered at the edge (WPT 2d.path.arc.scale.1, exact
/// pixels). A path far larger than the canvas keeps its curves (the segment count grows with the
/// square root of the size).
fn fine_flatten(path: BezPath, width: u32, height: u32) -> BezPath {
    if !path.elements().iter().any(|el| matches!(el, PathEl::QuadTo(..) | PathEl::CurveTo(..))) {
        return path;
    }
    let bounds = path.bounding_box();
    let limit = 16.0 * f64::from(width.max(height)).max(256.0);
    if !(bounds.width() <= limit && bounds.height() <= limit) {
        return path;
    }
    let mut out = BezPath::new();
    kurbo::flatten(path.elements().iter().copied(), 0.01, |el| out.push(el));
    out
}

/// Dash periods along one stroke past which dashes are dropped.
const MAX_DASHES: f64 = 100_000.0;

/// The length of `path`'s control polygon: at least its length, in O(n).
fn control_length(path: &BezPath) -> f64 {
    let mut last = Point::ZERO;
    let mut start = Point::ZERO;
    let mut length = 0.0;
    let mut to = |p: Point, last: &mut Point| {
        length += last.distance(p);
        *last = p;
    };
    for el in path.elements() {
        match *el {
            PathEl::MoveTo(p) => {
                last = p;
                start = p;
            }
            PathEl::LineTo(p) => to(p, &mut last),
            PathEl::QuadTo(a, b) => {
                to(a, &mut last);
                to(b, &mut last);
            }
            PathEl::CurveTo(a, b, c) => {
                to(a, &mut last);
                to(b, &mut last);
                to(c, &mut last);
            }
            PathEl::ClosePath => to(start, &mut last),
        }
    }
    length
}

fn invertible(t: Affine) -> bool {
    let det = t.determinant();
    det != 0.0 && det.is_finite() && t.as_coeffs().iter().all(|v| v.is_finite())
}

/// Sets the paint for one draw and issues it, inside the clipping region.
#[allow(clippy::too_many_arguments)]
fn emit(
    ctx: &mut RenderContext,
    resources: &mut Resources,
    paint: &Paint,
    state: &DrawState<'_>,
    geometry: Affine,
    alpha: f32,
    clip: &[ClipPath],
    (w, h): (u16, u16),
    draw: &mut dyn FnMut(&mut RenderContext, &mut Resources),
) {
    if !invertible(geometry) {
        return;
    }
    let t = state.transform;
    // The paint is in the user space of the transform; vello applies the
    // geometry's transform, then the paint transform.
    let paint_transform = geometry.inverse() * t;
    let mut clips = 0;
    ctx.reset_transform();
    ctx.reset_paint_transform();
    for c in clip {
        ctx.set_fill_rule(if c.evenodd { Fill::EvenOdd } else { Fill::NonZero });
        ctx.push_clip_path(&guard_path(&c.path, u32::from(w), u32::from(h)));
        clips += 1;
    }
    let drawn = match paint {
        Paint::Color(color) => {
            ctx.set_paint(color.multiply_alpha(alpha));
            true
        }
        Paint::Gradient(g) => match gradient_paint(g, alpha) {
            Some(gradient) if invertible(t) => {
                ctx.set_paint(PaintType::Gradient(gradient));
                ctx.set_paint_transform(paint_transform);
                true
            }
            _ => false,
        },
        Paint::Pattern(p) => {
            let to_device = t * p.transform;
            if invertible(to_device) {
                let quality = if state.smoothing { ImageQuality::Medium } else { ImageQuality::Low };
                let extend = |repeat| if repeat { Extend::Repeat } else { Extend::Pad };
                let image = Image {
                    image: ImageSource::Pixmap(p.image.pixmap().clone()),
                    sampler: ImageSampler {
                        x_extend: extend(p.repeat_x),
                        y_extend: extend(p.repeat_y),
                        quality,
                        alpha,
                    },
                };
                // A pattern that does not repeat on an axis is transparent
                // past its image there: clip to the image's band.
                if !p.repeat_x || !p.repeat_y {
                    let inverse = to_device.inverse();
                    let canvas = inverse.transform_rect_bbox(Rect::new(0.0, 0.0, f64::from(w), f64::from(h)));
                    let (iw, ih) = (f64::from(p.image.width()), f64::from(p.image.height()));
                    let band = Rect::new(
                        if p.repeat_x { canvas.x0.min(0.0) } else { 0.0 },
                        if p.repeat_y { canvas.y0.min(0.0) } else { 0.0 },
                        if p.repeat_x { canvas.x1.max(iw) } else { iw },
                        if p.repeat_y { canvas.y1.max(ih) } else { ih },
                    );
                    ctx.set_fill_rule(Fill::NonZero);
                    ctx.push_clip_path(&(to_device * band.to_path(0.1)));
                    clips += 1;
                }
                ctx.set_paint(PaintType::Image(image));
                ctx.set_paint_transform(geometry.inverse() * to_device);
                true
            } else {
                false
            }
        }
    };
    if drawn {
        ctx.set_transform(geometry);
        draw(ctx, resources);
    }
    ctx.reset_transform();
    for _ in 0..clips {
        ctx.pop_clip();
    }
}

/// The vello gradient of a canvas gradient with `alpha` applied, `None`
/// when it paints nothing (no stops, or a degenerate geometry).
fn gradient_paint(g: &Gradient, alpha: f32) -> Option<peniko::Gradient> {
    if g.stops.is_empty() {
        return None;
    }
    let mut stops: Vec<(f32, Color)> = g.stops.iter().map(|(o, c)| (*o, c.multiply_alpha(alpha))).collect();
    // Stable: stops at one offset keep their order (a hard transition).
    stops.sort_by(|a, b| a.0.total_cmp(&b.0));
    let gradient = match g.kind {
        GradientKind::Linear { start, end } => {
            if start == end {
                return None;
            }
            peniko::Gradient::new_linear(start, end)
        }
        GradientKind::Radial { start, start_radius, end, end_radius } => {
            if start == end && start_radius == end_radius {
                return None;
            }
            peniko::Gradient::new_two_point_radial(start, start_radius as f32, end, end_radius as f32)
        }
        GradientKind::Conic { center, angle } => {
            let a = angle as f32;
            // pyro: vello measures the angle in [0, 2π) and pads outside [start, start + 2π],
            // so any start other than 0 left part of the turn at an end stop (WPT
            // 2d.gradient.conic.positive.rotation); a conic gradient is one full turn from any
            // start, which repeating gives.
            peniko::Gradient::new_sweep(center, a, a + std::f32::consts::TAU).with_extend(Extend::Repeat)
        }
    };
    let mut gradient = gradient.with_stops(&stops[..]);
    gradient.interpolation_alpha_space = peniko::InterpolationAlphaSpace::Unpremultiplied;
    if g.oklab {
        gradient.interpolation_cs = peniko::color::ColorSpaceTag::Oklab;
    }
    Some(gradient)
}

/// Straight to premultiplied, rounding as browsers do.
pub fn premultiply(data: &mut [u8]) {
    for p in data.chunks_exact_mut(4) {
        let a = u32::from(p[3]);
        if a == 255 {
            continue;
        }
        for c in &mut p[..3] {
            *c = ((u32::from(*c) * a + 127) / 255) as u8;
        }
    }
}

/// Premultiplied to straight, rounding as browsers do.
pub fn unpremultiply(data: &mut [u8]) {
    for p in data.chunks_exact_mut(4) {
        let a = u32::from(p[3]);
        match a {
            255 => {}
            0 => p[..3].fill(0),
            _ => {
                for c in &mut p[..3] {
                    *c = ((u32::from(*c) * 255 + a / 2) / a).min(255) as u8;
                }
            }
        }
    }
}

/// A pixmap from premultiplied RGBA rows (a rasterized SVG image).
pub fn pixmap_from_premultiplied(data: Vec<u8>, width: u32, height: u32) -> Option<Pixmap> {
    if !valid_size(width, height) || data.len() != width as usize * height as usize * 4 {
        return None;
    }
    Some(Pixmap::from_parts(
        data,
        width as u16,
        height as u16,
        vello_cpu::PixelMetadata::new(peniko::ImageAlphaType::AlphaPremultiplied, true),
    ))
}

/// A pixmap from straight RGBA rows (`ImageData`, decoded images).
pub fn pixmap_from_straight(mut data: Vec<u8>, width: u32, height: u32) -> Option<Pixmap> {
    if !valid_size(width, height) || data.len() != width as usize * height as usize * 4 {
        return None;
    }
    premultiply(&mut data);
    Some(Pixmap::from_parts(
        data,
        width as u16,
        height as u16,
        vello_cpu::PixelMetadata::new(peniko::ImageAlphaType::AlphaPremultiplied, true),
    ))
}

#[cfg(test)]
mod tests;
