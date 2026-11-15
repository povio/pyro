//! HTML "select an image source" for an `<img>`: the matching `<source>` of
//! a `<picture>` parent, else the image's own `srcset` (with `sizes` giving
//! the source size of width descriptors), else its `src`.

use crate::dom::Dom;
use crate::media::Viewport;
use crate::node::{NodeId, NIL};

/// One image candidate of a `srcset`: its URL and pixel density.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub url: String,
    pub density: f64,
}

/// HTML "parse a srcset attribute". A width descriptor becomes a density
/// against `source_width` (CSS px; `sizes` is not read, so the default
/// `100vw` applies). Candidates with an invalid descriptor are dropped.
pub fn parse_srcset(input: &str, source_width: f64) -> Vec<Candidate> {
    let is_space = |c: char| matches!(c, ' ' | '\t' | '\n' | '\x0c' | '\r');
    let mut out = Vec::new();
    let mut rest = input;
    loop {
        rest = rest.trim_start_matches(|c| is_space(c) || c == ',');
        if rest.is_empty() {
            return out;
        }
        let end = rest.find(is_space).unwrap_or(rest.len());
        let (mut url, mut tail) = rest.split_at(end);
        let descriptors = if url.ends_with(',') {
            // The URL's trailing commas end the candidate.
            url = url.trim_end_matches(',');
            ""
        } else {
            tail = tail.trim_start_matches(is_space);
            let stop = tail.find(',').unwrap_or(tail.len());
            let (d, after) = tail.split_at(stop);
            tail = after;
            d
        };
        rest = tail;
        let mut density = None;
        let mut width = None;
        let mut valid = true;
        for token in descriptors.split(is_space).filter(|t| !t.is_empty()) {
            let (number, unit) = token.split_at(token.len() - 1);
            match unit {
                "x" if density.is_none() && width.is_none() => match number.parse::<f64>() {
                    Ok(v) if v >= 0.0 => density = Some(v),
                    _ => valid = false,
                },
                "w" if density.is_none() && width.is_none() => match number.parse::<u32>() {
                    Ok(v) if v > 0 => width = Some(v),
                    _ => valid = false,
                },
                // A height descriptor is parsed and ignored.
                "h" if number.parse::<u32>().is_ok() => {}
                _ => valid = false,
            }
        }
        if !valid || url.is_empty() {
            continue;
        }
        let density = match (density, width) {
            (Some(d), _) => d,
            (None, Some(w)) if source_width > 0.0 => f64::from(w) / source_width,
            _ => 1.0,
        };
        out.push(Candidate { url: url.to_owned(), density });
    }
}

/// The candidate for a device pixel ratio: the lowest density that is at
/// least `dpr`, else the highest.
pub fn pick_candidate(candidates: &[Candidate], dpr: f64) -> Option<&Candidate> {
    let enough = candidates.iter().filter(|c| c.density >= dpr).min_by(|a, b| a.density.total_cmp(&b.density));
    enough.or_else(|| candidates.iter().max_by(|a, b| a.density.total_cmp(&b.density)))
}

impl Dom {
    /// The URL (as written) the `<picture>` parent of `img` selects for it:
    /// the first preceding `<source>` sibling with a `srcset`, whose `media`
    /// matches `viewport` and whose `type`, when given, is an image type.
    /// `None` when `img` is not a picture's child or no source applies.
    pub fn picture_source(&self, img: NodeId, viewport: &Viewport) -> Option<String> {
        self.picture_candidate(img, viewport).map(|c| c.url)
    }

    /// [`Dom::picture_source`] with the candidate's pixel density (the image's natural size is
    /// its pixel size divided by it).
    pub fn picture_candidate(&self, img: NodeId, viewport: &Viewport) -> Option<Candidate> {
        let i = self.check(img).ok()?;
        let parent = self.n(i).parent;
        let html =
            |idx: u32, local: &str| self.n(idx).element().is_some_and(|e| e.is_html() && &*e.name.local == local);
        if parent == NIL || !html(parent, "picture") {
            return self.own_srcset_source(i, viewport);
        }
        let mut child = self.n(parent).first_child;
        while child != NIL && child != i {
            if html(child, "source") {
                let e = self.n(child).element().expect("source element");
                let media_ok = e
                    .attr_local(&markup5ever::local_name!("media"))
                    .is_none_or(|m| m.trim().is_empty() || crate::media::matches(m, viewport));
                let type_ok = e.attr_local(&markup5ever::local_name!("type")).is_none_or(|t| {
                    let t = t.trim().to_ascii_lowercase();
                    t.is_empty() || t.starts_with("image/")
                });
                if media_ok && type_ok {
                    if let Some(srcset) = e.attr_local(&markup5ever::local_name!("srcset")) {
                        let candidates = parse_srcset(
                            srcset,
                            source_size(e.attr_local(&markup5ever::local_name!("sizes")), viewport),
                        );
                        if let Some(c) = pick_candidate(&candidates, viewport.device_scale_factor) {
                            return Some(c.clone());
                        }
                    }
                }
            }
            child = self.n(child).next;
        }
        self.own_srcset_source(i, viewport)
    }

    /// The candidate the `<img>`'s own `srcset` selects; its `src` counts as
    /// a 1x candidate when the set has no 1x one and no width descriptors.
    fn own_srcset_source(&self, img: u32, viewport: &Viewport) -> Option<Candidate> {
        let e = self.n(img).element()?;
        let srcset = e.attr_local(&markup5ever::local_name!("srcset"))?;
        let mut candidates =
            parse_srcset(srcset, source_size(e.attr_local(&markup5ever::local_name!("sizes")), viewport));
        let has_width = srcset_has_width_descriptor(srcset);
        if let Some(src) = e.attr_local(&markup5ever::local_name!("src")).filter(|src| !src.is_empty()) {
            if !has_width && !candidates.iter().any(|c| c.density == 1.0) {
                candidates.push(Candidate { url: src.to_owned(), density: 1.0 });
            }
        }
        pick_candidate(&candidates, viewport.device_scale_factor).cloned()
    }
}

/// Whether a candidate of `srcset` has a width descriptor.
fn srcset_has_width_descriptor(srcset: &str) -> bool {
    srcset.split(',').any(|candidate| {
        candidate
            .split_ascii_whitespace()
            .skip(1)
            .any(|d| d.strip_suffix('w').is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())))
    })
}

/// HTML "parse a sizes attribute": the source size in CSS px. Each
/// comma-separated entry is a media condition followed by a length; the
/// first whose condition matches (or that has none) wins. `100vw` when
/// nothing applies.
pub fn source_size(sizes: Option<&str>, viewport: &Viewport) -> f64 {
    let default = f64::from(viewport.width);
    let Some(sizes) = sizes else { return default };
    let cx = crate::computed::LengthContext {
        viewport_width: f64::from(viewport.width),
        viewport_height: f64::from(viewport.height),
        ..crate::computed::LengthContext::fonts(16.0, 16.0)
    };
    // Split on top-level commas.
    let mut entries = Vec::new();
    let (mut depth, mut start) = (0i32, 0);
    for (i, ch) in sizes.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth <= 0 => {
                entries.push(&sizes[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    entries.push(&sizes[start..]);
    for entry in entries {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        // The size is the last component value: a token, or a function.
        let mut depth = 0i32;
        let mut split = 0;
        for (i, ch) in entry.char_indices().rev() {
            match ch {
                ')' => depth += 1,
                '(' => depth -= 1,
                c if c.is_ascii_whitespace() && depth == 0 => {
                    split = i + 1;
                    break;
                }
                _ => {}
            }
        }
        let (condition, size) = (entry[..split].trim(), &entry[split..]);
        // A percentage or a negative length is not a source size value.
        let Some((px, percent)) = crate::computed::resolve_len(size, &cx) else { continue };
        if percent != 0.0 || px < 0.0 || size.contains('%') {
            continue;
        }
        if condition.is_empty() || crate::media::matches(condition, viewport) {
            return px;
        }
    }
    default
}
