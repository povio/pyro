//! The text-decoration lines requested for the next insertion at a rich caret.
use crate::{editing, page::PageDom};
use pyro_dom::{stylo::declarations::DeclarationBlock, Dom, NodeId};

#[derive(Clone, Copy, Default, PartialEq)]
pub(super) struct Lines {
    pub underline: bool,
    pub strike: bool,
    other: bool,
}

impl Lines {
    fn parse(value: &str) -> Self {
        let mut lines = Self::default();
        for token in value.split_ascii_whitespace() {
            match token {
                "underline" => lines.underline = true,
                "line-through" => lines.strike = true,
                "overline" | "blink" => lines.other = true,
                _ => {}
            }
        }
        lines
    }

    pub(super) fn at(page: &PageDom, dom: &Dom, node: NodeId) -> Self {
        let layout = page.layout();
        let mut el = if dom.element(node).is_ok() { Some(node) } else { dom.parent_element(node).ok().flatten() };
        let mut lines = Self::default();
        while let Some(n) = el {
            let own = Self::parse(&layout.computed_style_value(dom, n, "text-decoration-line"));
            lines.underline |= own.underline;
            lines.strike |= own.strike;
            lines.other |= own.other;
            el = dom.parent_element(n).ok().flatten();
        }
        lines
    }

    pub(super) fn reported(self, authored: Self) -> Self {
        if self == Self::default() {
            return self;
        }
        Self {
            underline: self.underline || authored.underline,
            strike: self.strike || authored.strike,
            other: self.other || authored.other,
        }
    }

    pub(super) fn has(self, tag: &str) -> bool {
        if tag == "u" {
            self.underline
        } else {
            self.strike
        }
    }

    pub(super) fn toggle(&mut self, tag: &str) {
        if tag == "u" {
            self.underline = !self.underline;
        } else {
            self.strike = !self.strike;
        }
    }
}

/// Only semantic formatting tags and explicit inline decorations can be
/// removed by an editing command. A link or a stylesheet rule stays in effect.
fn clear_carriers(dom: &mut Dom, host: NodeId, typed: NodeId, wanted: Lines) -> Option<()> {
    let mut element = dom.parent_element(typed).ok()?;
    while let Some(n) = element {
        if n == host {
            break;
        }
        let parent = dom.parent_element(n).ok()??;
        element = Some(parent);
        let tag = editing::local(dom, n);
        let remove_tag = match tag.as_str() {
            "u" => !wanted.underline,
            "s" | "strike" => !wanted.strike,
            _ => false,
        };
        let mut style = DeclarationBlock::parse(dom.get_attribute(n, "style").ok()?.unwrap_or(""));
        let value = style.value("text-decoration-line");
        let own = Lines::parse(&value);
        let inline = matches!(tag.as_str(), "span" | "u" | "s" | "strike") && (own.underline || own.strike);
        if !remove_tag && !inline {
            continue;
        }
        let mut edges = [(typed, 0), (typed, editing::len(dom, typed))];
        let middle = super::alignment::split_edge(dom, parent, typed, false, &mut edges)?;
        super::alignment::split_edge(dom, parent, typed, true, &mut edges)?;
        if inline {
            let remaining: Vec<_> =
                value.split_whitespace().filter(|line| !matches!(*line, "underline" | "line-through")).collect();
            if remaining.is_empty() {
                style.remove("text-decoration");
            } else {
                style.set("text-decoration-line", &remaining.join(" "), false)?;
            }
            if style.is_empty() {
                dom.remove_attribute(middle, "style").ok()?;
            } else {
                dom.set_attribute(middle, "style", &style.css_text()).ok()?;
            }
        }
        if tag == "span" || remove_tag {
            if dom.attributes(middle).ok()?.is_empty() {
                super::indentation::unwrap(dom, middle, &mut edges)?;
            } else if tag != "span" {
                super::indentation::rename(dom, middle, "span", &mut edges)?;
            }
        }
    }
    Some(())
}

pub(super) fn apply(page: &PageDom, dom: &mut Dom, host: NodeId, typed: NodeId, wanted: Lines) -> Option<()> {
    clear_carriers(dom, host, typed, wanted)?;
    let active = Lines::at(page, dom, typed);
    let add_underline = wanted.underline && !active.underline;
    let add_strike = wanted.strike && !active.strike;
    if !add_underline && !add_strike {
        return Some(());
    }
    let doc = dom.node_document(typed).ok()?;
    if dom.document_data(doc).ok()?.style_with_css {
        let parent = dom.parent_element(typed).ok()??;
        let span = if parent != host && editing::local(dom, parent) == "span" && dom.child_count(parent).ok()? == 1 {
            parent
        } else {
            let span = dom.create_element(doc, "span").ok()?;
            dom.insert_before(parent, span, Some(typed)).ok()?;
            dom.append_child(span, typed).ok()?;
            span
        };
        let mut style = DeclarationBlock::parse(dom.get_attribute(span, "style").ok()?.unwrap_or(""));
        let existing = style.value("text-decoration-line");
        let mut lines: Vec<_> = existing.split_whitespace().filter(|&v| v != "none").collect();
        if add_underline {
            lines.push("underline");
        }
        if add_strike {
            lines.push("line-through");
        }
        style.set("text-decoration-line", &lines.join(" "), false)?;
        dom.set_attribute(span, "style", &style.css_text()).ok()?;
    } else {
        for (needed, tag) in [(add_underline, "u"), (add_strike, "strike")] {
            if needed {
                let parent = dom.parent_element(typed).ok()??;
                let el = dom.create_element(doc, tag).ok()?;
                dom.insert_before(parent, el, Some(typed)).ok()?;
                dom.append_child(el, typed).ok()?;
            }
        }
    }
    Some(())
}
