//! Where a page box goes on the paper when printing with `preferCSSPageSize: false`.
//!
//! Chromium (`pagination_utils.cc`, `TargetPageBorderBoxLogicalRect`, `kCenterShrinkToFitPaper`) keeps
//! the paper and, for a page with a fixed `@page` size, shrinks the box to fit it if needed and centers
//! it. The paper it centers on is whole points; the paint offset is then snapped to whole CSS px.
//! Measured on Chrome 156: a 210 x 297 mm page on A4 (596 x 843 pt) is 0.48 px from the left, which
//! snaps to 0, and 0.74 px from the top, which snaps to 1. A page without a fixed size
//! is not moved. A box of the other orientation turns the paper.
//!
//! Stylo's servo build has neither `size` nor `page`, so the rules are read from the text of the
//! `<style>` elements and `<link>` sheets (`@import`ed sheets are not seen), and the name from a `style`
//! attribute.

use crate::pdf;

/// The fixed `@page` size (CSS px) of a page named `name`, from CSS text in `media` (`print`, unless
/// emulated otherwise). Important declarations win, then named rules, then the later rule, as Chrome 156
/// resolves them: `@media print{@page{size:auto!important}}` overrides a named size in print media and is
/// ignored under an emulated screen, where Paprize's documents print.
pub(crate) fn fixed_page_size<'a>(
    sheets: impl Iterator<Item = &'a str>,
    name: Option<&str>,
    print_media: bool,
) -> Option<(f64, f64)> {
    let mut winner: Option<((bool, bool), Option<(f64, f64)>)> = None;
    for css in sheets {
        each_page_rule(css, print_media, &mut |selector, block| {
            let named = !selector.is_empty();
            if named && Some(selector) != name {
                return;
            }
            for (size, important) in block.split(';').filter_map(size_declaration) {
                if winner.is_none_or(|(rank, _)| (important, named) >= rank) {
                    winner = Some(((important, named), size));
                }
            }
        });
    }
    winner.and_then(|(_, size)| size)
}

/// Calls `f` with the selector and block of every `@page` rule of `css` whose enclosing `@media` rules
/// apply. Only the media type decides: `@media print` and `@media screen` by name, anything else applies.
fn each_page_rule(css: &str, print_media: bool, f: &mut impl FnMut(&str, &str)) {
    let applies = |query: &str| {
        let query = query.to_ascii_lowercase();
        query.split(',').any(|part| {
            let (print, screen) = (part.contains("print"), part.contains("screen"));
            let negated = part.contains("not ");
            match (print, screen) {
                (true, false) => print_media != negated,
                (false, true) => print_media == negated,
                _ => true,
            }
        })
    };
    // An `@media` we are in: skip its block when it does not apply.
    let mut rest = css;
    while let Some(at) = rest.find('@') {
        rest = &rest[at..];
        let Some(open) = rest.find('{') else { return };
        if let Some(query) = rest.strip_prefix("@media") {
            let query = &query[..open - "@media".len()];
            if applies(query) {
                rest = &rest[open + 1..];
            } else {
                rest = skip_block(&rest[open..]);
            }
        } else if let Some(selector) = rest.strip_prefix("@page") {
            let selector = selector[..open - "@page".len()].trim();
            let Some(close) = rest[open..].find('}') else { return };
            f(selector, &rest[open + 1..open + close]);
            rest = &rest[open + close + 1..];
        } else {
            rest = &rest[1..];
        }
    }
}

/// What follows the block `css` starts with.
fn skip_block(css: &str) -> &str {
    let mut depth = 0;
    for (i, c) in css.char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return &css[i + 1..];
                }
            }
            _ => {}
        }
    }
    ""
}

/// A `size` declaration: its fixed size (`None`: `auto`, or an orientation) and whether it is important.
fn size_declaration(declaration: &str) -> Option<(Option<(f64, f64)>, bool)> {
    let (property, value) = declaration.split_once(':')?;
    if property.trim() != "size" {
        return None;
    }
    let value = value.trim();
    let (value, important) = match value.strip_suffix("!important") {
        Some(value) => (value.trim(), true),
        None => (value, false),
    };
    let mut parts = value.split_whitespace();
    let size = parts.next().and_then(length).and_then(|width| {
        let height = parts.next().map_or(Some(width), length)?;
        parts.next().is_none().then_some((width, height))
    });
    Some((size, important))
}

fn length(token: &str) -> Option<f64> {
    let digits = token.find(|c: char| !(c.is_ascii_digit() || c == '.'))?;
    let value: f64 = token[..digits].parse().ok()?;
    let per_unit = match &token[digits..] {
        "px" => 1.0,
        "pt" => 96.0 / 72.0,
        "pc" => 16.0,
        "in" => 96.0,
        "cm" => 96.0 / 2.54,
        "mm" => 96.0 / 25.4,
        "q" | "Q" => 96.0 / 101.6,
        _ => return None,
    };
    Some(value * per_unit)
}

/// The `page` of a `style` attribute.
pub(crate) fn page_name(style_attribute: &str) -> Option<&str> {
    style_attribute.split(';').find_map(|declaration| {
        let (property, value) = declaration.split_once(':')?;
        (property.trim() == "page").then(|| value.trim())
    })
}

/// A page box of fixed size `source` (CSS px, if any) on `paper` (CSS px).
pub(crate) fn placement(paper: (f64, f64), source: Option<(f64, f64)>) -> pdf::Placement {
    let Some((width, height)) = source else {
        return pdf::Placement {
            paper: (paper.0 * pdf::PX_TO_PT, paper.1 * pdf::PX_TO_PT),
            offset: (0.0, 0.0),
            scale: 1.0,
        };
    };
    let turned = width != height && (width > height) != (paper.0 > paper.1);
    let paper = if turned { (paper.1, paper.0) } else { paper };
    // The written paper is on a 300 dpi grid; Chromium centers on the whole points before it.
    let whole_points = |px: f64| (px * pdf::PX_TO_PT).round() / pdf::PX_TO_PT;
    let target = (whole_points(paper.0), whole_points(paper.1));
    let scale = (target.0 / width).min(target.1 / height).min(1.0);
    let to_css_px = |free: f64| (free / 2.0).round().max(0.0);
    pdf::Placement {
        paper: (paper.0 * pdf::PX_TO_PT, paper.1 * pdf::PX_TO_PT),
        offset: (
            to_css_px(target.0 - width * scale) * pdf::PX_TO_PT,
            to_css_px(target.1 - height * scale) * pdf::PX_TO_PT,
        ),
        scale,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A4_PAPER: (f64, f64) = (595.92 / 0.75, 842.88 / 0.75);
    const LETTER: (f64, f64) = (816.0, 1056.0);

    /// Offset (CSS px) and scale; every expectation is from a PDF printed by Chrome 156.
    fn at(paper: (f64, f64), source: (f64, f64)) -> (f64, f64, f64, (f64, f64)) {
        let p = placement(paper, Some(source));
        (p.offset.0 / 0.75, p.offset.1 / 0.75, p.scale, p.paper)
    }

    #[test]
    fn placement_matches_chrome() {
        let mm = 96.0 / 25.4;
        // (210 mm x 297 mm is what ToS CMR documents ask for)
        let (x, y, s, paper) = at(A4_PAPER, (210.0 * mm, 297.0 * mm));
        assert_eq!((x, y, s), (0.0, 1.0, 1.0));
        assert!((paper.0 - 595.92).abs() < 1e-6 && (paper.1 - 842.88).abs() < 1e-6);
        let pt = 96.0 / 72.0;
        // Paprize's A4 of the other ToS documents.
        assert_eq!(at(A4_PAPER, (595.92 * pt, 841.92 * pt)).1, 1.0);
        assert_eq!(at(A4_PAPER, (595.92 * pt, 841.92 * pt)).0, 0.0);
        // Smaller boxes center.
        let (x, y, s, _) = at(A4_PAPER, (500.0, 700.0));
        assert_eq!((x, y, s), (147.0, 212.0, 1.0));
        let (x, y, s, _) = at(LETTER, (500.0, 700.0));
        assert_eq!((x, y, s), (158.0, 178.0, 1.0));
        // Larger boxes shrink: 2.4833496 / 3.125 at 0, 6 px; 2.9398255 / 3.125 at 35, 0.
        let (x, y, s, _) = at(A4_PAPER, (1000.0, 1400.0));
        assert!((s - 2.4833496 / 3.125).abs() < 1e-5 && x == 0.0 && y == 6.0, "{x} {y} {s}");
        let (x, y, s, _) = at(LETTER, (210.0 * mm, 297.0 * mm));
        assert!((s - 2.9398255 / 3.125).abs() < 1e-5 && x == 35.0 && y == 0.0, "{x} {y} {s}");
        // A landscape box turns the paper (bmd-report's 841.92 x 595.92 pt, on portrait paper).
        let (x, y, s, paper) = at(A4_PAPER, (297.0 * mm, 210.0 * mm));
        assert_eq!((x, y, s), (1.0, 0.0, 1.0));
        assert!((paper.0 - 842.88).abs() < 1e-6 && (paper.1 - 595.92).abs() < 1e-6);
    }

    #[test]
    fn a_page_without_a_fixed_size_stays_at_the_corner() {
        let p = placement(A4_PAPER, None);
        assert_eq!((p.offset, p.scale), ((0.0, 0.0), 1.0));
    }

    #[test]
    fn page_rules_cascade_as_in_chrome() {
        let size = |css: &str, name: Option<&str>, print: bool| fixed_page_size([css].into_iter(), name, print);
        let paprize = "@media print{@page{margin:0;size:auto!important}.a{b:c}} .x{y:z} @page s{margin:0;size:210mm 297mm;width:210mm}";
        // Printed under an emulated screen, as ToS does.
        let got = size(paprize, Some("s"), false).unwrap();
        assert!((got.0 - 793.700787).abs() < 1e-3 && (got.1 - 1122.519685).abs() < 1e-3);
        assert_eq!(size(paprize, Some("s"), true), None);
        assert_eq!(size(paprize, None, false), None);
        assert_eq!(size("@page{size:auto!important}@page s{size:500px 700px}", Some("s"), false), None);
        assert_eq!(size("@page{size:500px 700px}", None, true), Some((500.0, 700.0)));
        assert_eq!(size("@page{size:500px}", Some("x"), true), Some((500.0, 500.0)));
        assert_eq!(size("@page{margin:0}", None, true), None);
        assert_eq!(size("@media screen{@page{size:1px 2px}}", None, true), None);
        assert_eq!(size("@media screen{@page{size:1px 2px}}", None, false), Some((1.0, 2.0)));
    }

    #[test]
    fn page_names_come_from_the_style_attribute() {
        assert_eq!(page_name("page: section-__0001;"), Some("section-__0001"));
        assert_eq!(page_name("width:1px; page :x"), Some("x"));
        assert_eq!(page_name("page-break-after:always"), None);
    }
}
