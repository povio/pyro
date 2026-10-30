//! Display-list hash: equal hashes mean the same pixels for one backend, so a
//! match whose hash equals the baseline's skips raster and compare.

use std::collections::HashMap;
use std::sync::Arc;

use anyrender::recording::{RenderCommand, Scene};
use anyrender::Paint;
use kurbo::{Affine, BezPath, PathEl};
use peniko::Color;
use xxhash_rust::xxh3::{xxh3_64, Xxh3};

/// Hash of every command of `scene` and the image size. Glyph fonts are
/// hashed by content (memoized per blob in `font_hashes`), so the hash is
/// stable across processes and can be stored with a baseline.
pub fn scene_hash(scene: &Scene, width: u32, height: u32, font_hashes: &mut HashMap<u64, u64>) -> u64 {
    let mut h = Xxh3::new();
    h.update(&width.to_le_bytes());
    h.update(&height.to_le_bytes());
    for cmd in &scene.commands {
        match cmd {
            RenderCommand::PushLayer(c) => {
                h.update(&[1, c.fill as u8, c.blend.mix as u8, c.blend.compose as u8]);
                h.update(&c.alpha.to_bits().to_le_bytes());
                affine(&mut h, &c.transform);
                path(&mut h, &c.clip);
                if c.filter.is_some() || c.backdrop_filter.is_some() {
                    h.update(format!("{:?}{:?}", c.filter, c.backdrop_filter).as_bytes());
                }
            }
            RenderCommand::PushClipLayer(c) => {
                h.update(&[2, c.fill as u8]);
                affine(&mut h, &c.transform);
                path(&mut h, &c.clip);
            }
            RenderCommand::PopLayer => h.update(&[3]),
            RenderCommand::Stroke(c) => {
                h.update(&[4, c.style.join as u8, c.style.start_cap as u8, c.style.end_cap as u8]);
                for v in [c.style.width, c.style.miter_limit, c.style.dash_offset] {
                    h.update(&v.to_bits().to_le_bytes());
                }
                for v in c.style.dash_pattern.iter() {
                    h.update(&v.to_bits().to_le_bytes());
                }
                affine(&mut h, &c.transform);
                paint(&mut h, &c.brush);
                opt_affine(&mut h, &c.brush_transform);
                path(&mut h, &c.shape);
            }
            RenderCommand::Fill(c) => {
                h.update(&[5, c.fill as u8]);
                affine(&mut h, &c.transform);
                paint(&mut h, &c.brush);
                opt_affine(&mut h, &c.brush_transform);
                path(&mut h, &c.shape);
            }
            RenderCommand::GlyphRun(c) => {
                h.update(&[6, c.hint as u8]);
                let blob = &c.font_data.data;
                let fh = *font_hashes.entry(blob.id()).or_insert_with(|| xxh3_64(blob.data()));
                h.update(&fh.to_le_bytes());
                h.update(&c.font_data.index.to_le_bytes());
                h.update(&c.font_size.to_bits().to_le_bytes());
                for v in &c.normalized_coords {
                    h.update(&v.to_le_bytes());
                }
                h.update(&c.embolden.x.to_bits().to_le_bytes());
                h.update(&c.embolden.y.to_bits().to_le_bytes());
                match &c.style {
                    peniko::Style::Fill(f) => h.update(&[0, *f as u8]),
                    s => h.update(format!("{s:?}").as_bytes()),
                }
                paint(&mut h, &c.brush);
                h.update(&c.brush_alpha.to_bits().to_le_bytes());
                affine(&mut h, &c.transform);
                opt_affine(&mut h, &c.glyph_transform);
                for g in &c.glyphs {
                    h.update(&g.id.to_le_bytes());
                    h.update(&g.x.to_bits().to_le_bytes());
                    h.update(&g.y.to_bits().to_le_bytes());
                }
            }
            RenderCommand::BoxShadow(c) => {
                h.update(&[7]);
                affine(&mut h, &c.transform);
                for v in [c.rect.x0, c.rect.y0, c.rect.x1, c.rect.y1, c.radius, c.std_dev] {
                    h.update(&v.to_bits().to_le_bytes());
                }
                color(&mut h, &c.brush);
            }
        }
    }
    h.digest()
}

fn affine(h: &mut Xxh3, a: &Affine) {
    for v in a.as_coeffs() {
        h.update(&v.to_bits().to_le_bytes());
    }
}

fn opt_affine(h: &mut Xxh3, a: &Option<Affine>) {
    match a {
        Some(a) => {
            h.update(&[1]);
            affine(h, a);
        }
        None => h.update(&[0]),
    }
}

fn color(h: &mut Xxh3, c: &Color) {
    for v in c.components {
        h.update(&v.to_bits().to_le_bytes());
    }
}

fn paint(h: &mut Xxh3, p: &Paint) {
    match p {
        Paint::Solid(c) => {
            h.update(&[0]);
            color(h, c);
        }
        Paint::Gradient(g) => {
            h.update(&[1]);
            h.update(format!("{g:?}").as_bytes());
        }
        Paint::Image(i) => {
            h.update(&[2]);
            h.update(&xxh3_64(i.image.data.data()).to_le_bytes());
            h.update(format!("{:?}", i.sampler).as_bytes());
        }
        Paint::Resource(r) => {
            h.update(&[3]);
            h.update(format!("{r:?}").as_bytes());
        }
        // Opaque backend-specific paint: hash identity only.
        Paint::Custom(c) => {
            h.update(&[4]);
            h.update(&(Arc::as_ptr(c) as *const u8 as usize).to_le_bytes());
        }
    }
}

fn path(h: &mut Xxh3, p: &BezPath) {
    for el in p.elements() {
        let pts: &[kurbo::Point] = match el {
            PathEl::MoveTo(p) => {
                h.update(&[0]);
                std::slice::from_ref(p)
            }
            PathEl::LineTo(p) => {
                h.update(&[1]);
                std::slice::from_ref(p)
            }
            PathEl::QuadTo(a, b) => {
                h.update(&[2]);
                for p in [a, b] {
                    h.update(&p.x.to_bits().to_le_bytes());
                    h.update(&p.y.to_bits().to_le_bytes());
                }
                &[]
            }
            PathEl::CurveTo(a, b, c) => {
                h.update(&[3]);
                for p in [a, b, c] {
                    h.update(&p.x.to_bits().to_le_bytes());
                    h.update(&p.y.to_bits().to_le_bytes());
                }
                &[]
            }
            PathEl::ClosePath => {
                h.update(&[4]);
                &[]
            }
        };
        for p in pts {
            h.update(&p.x.to_bits().to_le_bytes());
            h.update(&p.y.to_bits().to_le_bytes());
        }
    }
}
