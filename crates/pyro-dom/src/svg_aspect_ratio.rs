//! SVG 2 preserveAspectRatio parsing and the value reflected by its DOM interface.
use crate::{Dom, DomResult, NodeId};

const ALIGN: [&str; 10] = [
    "none", "xMinYMin", "xMidYMin", "xMaxYMin", "xMinYMid", "xMidYMid", "xMaxYMid", "xMinYMax", "xMidYMax", "xMaxYMax",
];

#[derive(Clone, Copy)]
pub struct PreserveAspectRatio {
    align: u16,
    slice: bool,
}

impl Default for PreserveAspectRatio {
    fn default() -> Self {
        Self { align: 6, slice: false } // xMidYMid meet
    }
}

impl PreserveAspectRatio {
    fn parse(text: &str) -> Option<Self> {
        let mut words = text.split([' ', '\t', '\r', '\n']).filter(|word| !word.is_empty());
        let keyword = words.next()?;
        let align = ALIGN.iter().position(|&s| s == keyword)? as u16 + 1;
        let slice = match words.next() {
            Some("slice") => true,
            None | Some("meet") => false,
            _ => return None,
        };
        words.next().is_none().then_some(Self { align, slice })
    }

    pub fn align(self) -> u16 {
        self.align
    }
    pub fn meet_or_slice(self) -> u16 {
        if self.slice { 2 } else { 1 }
    }

    pub fn with_align(self, align: u16) -> Option<Self> {
        (1..=10).contains(&align).then_some(Self { align, ..self })
    }

    pub fn with_meet_or_slice(self, mode: u16) -> Option<Self> {
        (1..=2).contains(&mode).then_some(Self { slice: mode == 2, ..self })
    }

    /// Fractions of the free width and height for each Min/Mid/Max pair.
    /// With "none" the two scales are independent.
    pub(crate) fn alignment(self) -> Option<(f64, f64)> {
        let index = self.align.checked_sub(2)?;
        Some((f64::from(index % 3) * 0.5, f64::from(index / 3) * 0.5))
    }
}

impl Dom {
    pub fn svg_aspect_ratio(&self, id: NodeId) -> DomResult<PreserveAspectRatio> {
        Ok(self
            .get_attribute_ns(id, None, "preserveAspectRatio")?
            .and_then(PreserveAspectRatio::parse)
            .unwrap_or_default())
    }

    pub fn set_svg_aspect_ratio(&mut self, id: NodeId, value: PreserveAspectRatio) -> DomResult<()> {
        let align = ALIGN[usize::from(value.align - 1)];
        let mode = if value.slice { "slice" } else { "meet" };
        self.set_attribute_ns(id, None, "preserveAspectRatio", &format!("{align} {mode}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DocumentKind;

    #[test]
    fn none_reflects_the_explicit_slice_keyword_without_preserving_the_geometry_ratio() {
        let mut dom = Dom::new();
        let doc = dom.create_document(DocumentKind::Html, "about:blank");
        let svg = dom.create_element_ns(doc, Some("http://www.w3.org/2000/svg"), "svg").unwrap();
        dom.set_attribute(svg, "preserveAspectRatio", "none slice").unwrap();
        let parsed = dom.svg_aspect_ratio(svg).unwrap();
        assert_eq!((parsed.align(), parsed.meet_or_slice()), (1, 2));
        assert_eq!(parsed.alignment(), None);
        dom.set_svg_aspect_ratio(svg, parsed.with_meet_or_slice(2).unwrap()).unwrap();
        dom.set_attribute_ns(svg, Some("urn:other"), "preserveAspectRatio", "xMaxYMax").unwrap();
        assert_eq!(dom.get_attribute_ns(svg, None, "preserveAspectRatio").unwrap(), Some("none slice"));
        assert_eq!(dom.svg_aspect_ratio(svg).unwrap().meet_or_slice(), 2);
        dom.set_attribute(svg, "preserveAspectRatio", "none slice").unwrap();
        assert_eq!(dom.svg_aspect_ratio(svg).unwrap().meet_or_slice(), 2);
        dom.remove_attribute(svg, "preserveAspectRatio").unwrap();
        assert_eq!(dom.svg_aspect_ratio(svg).unwrap().align(), 6);
    }
}
