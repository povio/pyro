//! `<picture>`: the image source a `<source>` sibling provides for an
//! `<img>` (HTML "select an image source", the part that reads the picture's
//! sources). The `<img>`'s own `srcset`/`sizes` are not modelled: without a
//! matching `<source>` the image loads its `src`.

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
        let i = self.check(img).ok()?;
        let parent = self.n(i).parent;
        let html = |idx: u32, local: &str| {
            self.n(idx).element().is_some_and(|e| e.is_html() && &*e.name.local == local)
        };
        if parent == NIL || !html(parent, "picture") {
            return None;
        }
        let mut child = self.n(parent).first_child;
        while child != NIL && child != i {
            if html(child, "source") {
                let e = self.n(child).element().expect("source element");
                let media_ok = e.attr("media").is_none_or(|m| m.trim().is_empty() || crate::media::matches(m, viewport));
                let type_ok = e.attr("type").is_none_or(|t| {
                    let t = t.trim().to_ascii_lowercase();
                    t.is_empty() || t.starts_with("image/")
                });
                if media_ok && type_ok {
                    if let Some(srcset) = e.attr("srcset") {
                        let candidates = parse_srcset(srcset, f64::from(viewport.width));
                        if let Some(c) = pick_candidate(&candidates, viewport.device_scale_factor) {
                            return Some(c.url.clone());
                        }
                    }
                }
            }
            child = self.n(child).next;
        }
        None
    }
}
