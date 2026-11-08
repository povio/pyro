//! Declarative refresh: `<meta http-equiv=refresh>` and the HTTP `Refresh`
//! response header (HTML "shared declarative refresh steps").
//!
//! - The first such pragma of a document wins ("will declaratively
//!   refresh"); a meta counts when it is inserted into the document tree
//!   (not a shadow tree), whether by the parser or by script. Removing it
//!   afterwards does not cancel the refresh. Changing its attributes later
//!   does nothing.
//! - The refresh comes due `time` seconds after the document's `load` event
//!   (or after the insertion, when that is later), then navigates the frame
//!   to the URL (the document's URL when none is given). As in Chromium, a
//!   delay of at most one second replaces the history entry.
//! - Frames sandboxed without `allow-scripts` do not refresh, nor does a
//!   refresh to a `javascript:` URL (as in Chromium).
//! - A navigation commits a new document, which drops the scheduled
//!   refresh: its timer finds a different loader and does nothing.
use lp_dom::NodeId;

use crate::frames::FrameKey;
use crate::{Browser, HistoryHandling, LoaderId, NavigateOpts, NavigationReason};

/// Per document.
#[derive(Default)]
pub(crate) struct Refresh {
    /// HTML "will declaratively refresh".
    claimed: bool,
    /// Parsed before `load`: seconds and URL, scheduled when it fires.
    pending: Option<(u64, String)>,
}

/// HTML "shared declarative refresh steps", parsing only: the delay in
/// seconds and the unresolved URL (`None` for the document's own URL).
pub(crate) fn parse(input: &str) -> Option<(u64, Option<&str>)> {
    let ws = |c: char| matches!(c, '\t' | '\n' | '\x0c' | '\r' | ' ');
    let mut rest = input.trim_start_matches(ws);
    let digits = rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(rest.len());
    let time = if digits == 0 {
        if !rest.starts_with('.') {
            return None;
        }
        0
    } else {
        // Saturates: a huge delay never comes due anyway.
        rest[..digits].parse::<u64>().unwrap_or(u64::MAX)
    };
    rest = &rest[digits..];
    rest = rest.trim_start_matches(|c: char| c.is_ascii_digit() || c == '.');
    if rest.is_empty() {
        return Some((time, None));
    }
    if !rest.starts_with([';', ',']) && !rest.starts_with(ws) {
        return None;
    }
    rest = rest.trim_start_matches(ws);
    if rest.starts_with([';', ',']) {
        rest = &rest[1..];
    }
    rest = rest.trim_start_matches(ws);
    if rest.is_empty() {
        return Some((time, None));
    }
    let url_string = rest;
    // `url` `=` prefix (each letter ASCII case-insensitive); without the
    // full prefix the remainder is the URL as it is.
    let prefixed = (|| {
        let mut r = rest;
        for letter in ['u', 'r', 'l'] {
            r = r.strip_prefix(letter).or_else(|| r.strip_prefix(letter.to_ascii_uppercase()))?;
        }
        let r = r.trim_start_matches(ws).strip_prefix('=')?;
        Some(r.trim_start_matches(ws))
    })();
    let rest = match prefixed {
        Some(r) => r,
        None if rest.starts_with(['u', 'U']) => return Some((time, Some(url_string))),
        None => rest,
    };
    // Skip quotes.
    let url = match rest.chars().next() {
        Some(q @ ('"' | '\'')) => {
            let inner = &rest[1..];
            inner.find(q).map_or(inner, |end| &inner[..end])
        }
        _ => rest,
    };
    Some((time, Some(url)))
}

impl Browser {
    /// A `<meta http-equiv=refresh>` was inserted into `page`'s document.
    pub(crate) fn meta_refresh_event(&mut self, page: FrameKey, node: NodeId) {
        let Some(bindings) = self.doc(page).and_then(|d| d.main_frame.page_dom.clone()) else { return };
        let content = {
            let dom = bindings.dom.borrow();
            // The DOM queued it inserted into the document tree; it may have
            // been removed since, which does not cancel the pragma.
            let content = dom.get_attribute(node, "content").ok().flatten().filter(|c| !c.is_empty());
            match content {
                Some(c) => c.to_owned(),
                None => return,
            }
        };
        let base = crate::loader::node_base_url(&bindings.dom.borrow(), node);
        self.declarative_refresh(page, &content, base);
    }

    /// The `Refresh` header of the response that made `page`'s document.
    pub(crate) fn refresh_header(&mut self, page: FrameKey, value: &str) {
        let base = self.doc(page).map(|d| d.main_frame.base_url.clone());
        self.declarative_refresh(page, value, base);
    }

    fn declarative_refresh(&mut self, page: FrameKey, input: &str, base: Option<String>) {
        let Some(mut doc) = self.doc_mut(page) else { return };
        if doc.refresh.claimed || doc.main_frame.sandbox.scripts_disabled {
            return;
        }
        let Some((time, url)) = parse(input) else { return };
        let document_url = doc.main_frame.url.clone();
        let target = match url {
            None => document_url,
            Some(u) => {
                let Some(base) = base.and_then(|b| url::Url::parse(&b).ok()) else { return };
                match base.join(u) {
                    Ok(u) => u.to_string(),
                    Err(_) => return,
                }
            }
        };
        // Chromium refuses a refresh to a `javascript:` URL (it does not
        // claim the document's refresh either).
        if target.get(..11).is_some_and(|p| p.eq_ignore_ascii_case("javascript:")) {
            return;
        }
        doc.refresh.claimed = true;
        let loader = doc.main_frame.loader_id.clone();
        if doc.main_frame.loaded {
            drop(doc);
            self.schedule_refresh(page, loader, time, target);
        } else {
            doc.refresh.pending = Some((time, target));
        }
    }

    /// The document's `load` event fired: a refresh parsed before it starts.
    pub(crate) fn start_pending_refresh(&mut self, page: FrameKey, loader: LoaderId) {
        let Some(pending) = self.doc_mut(page).and_then(|mut d| d.refresh.pending.take()) else { return };
        self.schedule_refresh(page, loader, pending.0, pending.1);
    }

    fn schedule_refresh(&mut self, page: FrameKey, loader: LoaderId, time: u64, url: String) {
        let delay = std::time::Duration::from_secs(time.min(u32::MAX as u64));
        // An immediate redirect keeps the loop busy (an interstitial is
        // left before the page counts as idle); a periodic auto-refresh
        // does not.
        self.scheduler().set_timer(delay, false, time <= 1, move |b| {
            let Some(doc) = b.doc(page).filter(|d| d.main_frame.loader_id == loader) else { return };
            let referrer = doc.main_frame.url.clone();
            drop(doc);
            let opts = NavigateOpts {
                post: None,
                url: url.clone(),
                reason: NavigationReason::Script,
                cdp_request_id: None,
                referrer: Some(referrer),
            };
            let history = if time <= 1 { HistoryHandling::Replace } else { HistoryHandling::Push };
            let _ = b.navigate_frame_with_handling(page, opts, history);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn shared_declarative_refresh_parsing() {
        // (input, expected) from WPT attr-meta-http-equiv-refresh/parsing.html.
        let ok = |input: &str, time: u64, url: Option<&str>| assert_eq!(parse(input), Some((time, url)), "{input:?}");
        let bad = |input: &str| assert_eq!(parse(input), None, "{input:?}");
        bad("");
        for i in ["1", "1 ", "1\t", "1\r", "1\n", "1\x0c", "1;", "1,"] {
            ok(i, 1, None);
        }
        for i in [
            "1; url=foo",
            "1, url=foo",
            "1 url=foo",
            "1;\turl=foo",
            "1\nurl=foo",
            "  1  ;  url  =  foo",
            "  1  ,  url  =  foo",
            "  1  ;  foo",
            "  1  url  =  foo",
            "1; url=\"foo\"bar",
            "1; url='foo'bar",
            "1; \"foo\"bar",
            "1.9; url=foo",
            "1.9..5.; url=foo",
        ] {
            ok(i, 1, Some("foo"));
        }
        // Trailing whitespace and inner tabs/newlines are left to the URL parser.
        ok("1; url=foo ", 1, Some("foo "));
        ok("1; url=\"foo'bar", 1, Some("foo'bar"));
        ok("1; url foo", 1, Some("url foo"));
        ok("1; urlfoo", 1, Some("urlfoo"));
        ok("1; urfoo", 1, Some("urfoo"));
        ok("1; ufoo", 1, Some("ufoo"));
        ok("1 x;url=foo", 1, Some("x;url=foo"));
        ok("1;;url=foo", 1, Some(";url=foo"));
        for i in [
            "1url=foo",
            "1x;url=foo",
            "; foo",
            ";foo",
            ", foo",
            ",foo",
            "foo",
            "+1; url=foo",
            "-1; url=foo",
            "+0",
            "-0",
        ] {
            bad(i);
        }
        for i in ["1e0; url=foo", "1e1; url=foo", "10e-1; url=foo", "-0.1; url=foo"] {
            bad(i);
        }
        ok("0; url=foo", 0, Some("foo"));
        ok("0", 0, None);
        ok(".9; url=foo", 0, Some("foo"));
        ok("0.9; url=foo", 0, Some("foo"));
        ok("0...; url=foo", 0, Some("foo"));
    }
}
