//! `DOMSnapshot.captureSnapshot`: the main frame's layout in Chromium's shape,
//! for comparing layouts (`bench/oracle/`). Nothing runs unless it is called;
//! a call brings layout up to date, as any layout query does.
//!
//! Compatible: `documents[0]` with `nodes` (`parentIndex`, `nodeType`,
//! `nodeName`, `nodeValue`, `attributes`, `pseudoType`), `layout` (`nodeIndex`,
//! `bounds`, `text`, `styles`, `stackingContexts`), `textBoxes` (`layoutIndex`,
//! `bounds`, `start`, `length`) and `strings`, with a `::before` or `::after`'s
//! generated text as extra layout rows of its node, as in Chromium.
//!
//! Different:
//! - Only the root element's subtree of the main frame: no document node, no
//!   comments, no other frames, no `::marker`, no `backendNodeId`.
//! - Values are unrounded (see `lp_render::LayoutSnapshot`), not layout units.
//! - `computedStyles` must be empty (use `getComputedStyle`); `styles` rows
//!   are empty. `paintOrders`, `includeDOMRects` and the color options are
//!   accepted and produce nothing.
//! - `lp` adds what Chromium does not report: the line boxes and, parallel to
//!   `textBoxes`, each fragment's baseline and font metrics.

use serde::Deserialize;
use serde_json::{json, Value};

use lp_render::SnapshotKind;

use super::{EngineHost, PageCommand};
use crate::msg::{CdpError, CdpResult};

const ELEMENT_NODE: u32 = 1;
const TEXT_NODE: u32 = 3;

#[derive(Default)]
struct Strings {
    table: Vec<String>,
    at: std::collections::HashMap<String, usize>,
}

impl Strings {
    fn add(&mut self, s: &str) -> i64 {
        if let Some(&i) = self.at.get(s) {
            return i as i64;
        }
        let i = self.table.len();
        self.table.push(s.to_owned());
        self.at.insert(s.to_owned(), i);
        i as i64
    }
}

impl EngineHost {
    pub(super) fn dom_snapshot_domain(&mut self, action: &str, cmd: &PageCommand<'_, '_>) -> CdpResult<Option<Value>> {
        match action {
            "enable" | "disable" => Ok(Some(json!({}))),
            "captureSnapshot" => self.capture_snapshot(cmd),
            _ => Err(CdpError::unsupported(&cmd.command.method)),
        }
    }

    fn capture_snapshot(&mut self, cmd: &PageCommand<'_, '_>) -> CdpResult<Option<Value>> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct P {
            computed_styles: Vec<String>,
        }
        let p: P = cmd.command.params()?;
        if !p.computed_styles.is_empty() {
            return Err(CdpError::invalid_params("computedStyles is not supported: use getComputedStyle"));
        }
        let id = self.pages.get(&cmd.page).ok_or_else(|| CdpError::server("page is gone"))?.id;
        let page = self.thread.browser.page(id).ok_or_else(|| CdpError::server("page is gone"))?;
        let url = page.nav.url();
        let bindings = page.main_frame.page_dom.as_ref().ok_or_else(|| CdpError::server("no document"))?;
        let snapshot = {
            let dom = bindings.dom.borrow();
            let layout = bindings.layout();
            lp_layout::layout_snapshot(&dom, &layout).ok_or_else(|| CdpError::server("no layout for this document"))?
        };
        let mut strings = Strings::default();
        let (mut parent_index, mut node_type, mut node_name, mut node_value, mut attributes) =
            (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
        let (mut pseudo_index, mut pseudo_value) = (Vec::new(), Vec::new());
        let (mut layout_node, mut bounds, mut layout_text, mut styles) =
            (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        let (mut fixed, mut text_layout, mut text_bounds, mut start, mut length) =
            (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
        let mut runs = Vec::new();
        // A snapshot node's index in `nodes` (a pseudo's text is rows of the pseudo's node).
        let mut node_at = vec![-1i64; snapshot.nodes.len()];
        for (i, n) in snapshot.nodes.iter().enumerate() {
            let in_pseudo = n.parent.is_some_and(|p| matches!(snapshot.nodes[p].kind, SnapshotKind::Pseudo(_)));
            let row_node = if in_pseudo {
                node_at[n.parent.expect("in a pseudo")]
            } else {
                let index = parent_index.len() as i64;
                parent_index.push(n.parent.map_or(-1, |p| node_at[p]));
                let empty = strings.add("");
                match &n.kind {
                    SnapshotKind::Element { name, attrs } => {
                        node_type.push(ELEMENT_NODE);
                        node_name.push(strings.add(&name.to_ascii_uppercase()));
                        node_value.push(empty);
                        attributes.push(
                            attrs.iter().flat_map(|(k, v)| [strings.add(k), strings.add(v)]).collect::<Vec<_>>(),
                        );
                    }
                    SnapshotKind::Text(t) => {
                        node_type.push(TEXT_NODE);
                        node_name.push(strings.add("#text"));
                        node_value.push(strings.add(t));
                        attributes.push(Vec::new());
                    }
                    SnapshotKind::Pseudo(name) => {
                        node_type.push(ELEMENT_NODE);
                        node_name.push(strings.add(name));
                        node_value.push(empty);
                        attributes.push(Vec::new());
                        pseudo_index.push(index);
                        pseudo_value.push(strings.add(name.trim_start_matches(':')));
                    }
                }
                index
            };
            node_at[i] = row_node;
            let Some(rect) = n.rect else { continue };
            let row = layout_node.len();
            layout_node.push(row_node);
            bounds.push(rect);
            styles.push(Vec::<i64>::new());
            if n.fixed {
                fixed.push(row);
            }
            let laid: String = n.text.iter().map(|t| t.text.as_str()).collect();
            layout_text.push(strings.add(&laid));
            for t in &n.text {
                text_layout.push(row);
                text_bounds.push(t.rect);
                start.push(t.start);
                length.push(t.length);
                runs.push(json!({
                    "text": strings.add(&t.text), "baseline": t.baseline, "ascent": t.ascent,
                    "descent": t.descent, "fontSize": t.font_size,
                }));
            }
        }
        let lines: Vec<Value> = snapshot
            .lines
            .iter()
            .map(|l| json!({ "bounds": l.rect, "baseline": l.baseline }))
            .collect();
        let empty = strings.add("");
        let url_index = strings.add(&url);
        Ok(Some(json!({
            "documents": [{
                "documentURL": url_index, "title": empty, "baseURL": url_index,
                "contentLanguage": empty, "encodingName": empty, "publicId": empty, "systemId": empty,
                "frameId": empty, "scrollOffsetX": 0, "scrollOffsetY": 0,
                "contentWidth": snapshot.document_width, "contentHeight": snapshot.document_height,
                "nodes": {
                    "parentIndex": parent_index, "nodeType": node_type, "nodeName": node_name,
                    "nodeValue": node_value, "attributes": attributes,
                    "pseudoType": { "index": pseudo_index, "value": pseudo_value },
                },
                "layout": {
                    "nodeIndex": layout_node, "styles": styles, "bounds": bounds, "text": layout_text,
                    "stackingContexts": { "index": [] },
                },
                "textBoxes": { "layoutIndex": text_layout, "bounds": text_bounds, "start": start, "length": length },
                "lp": { "fixedRows": fixed, "textRuns": runs, "lines": lines },
            }],
            "strings": strings.table,
        })))
    }
}
