//! Session history, `Location`, `History` and `Performance`.
//!
//! [`PageNav`] is the per-page source of truth for the current URL and the
//! joint session history. JS-facing classes read it synchronously; anything
//! that needs `&mut Browser` (cross-document navigation, events) is posted as
//! a task, as navigation is asynchronous in browsers too.
//!
//! Known gaps (see docs/PARITY.md): `popstate`/`hashchange` DOM events need
//! lp-webapi's EventTarget; history state is stored as JSON (not a full
//! structured clone); traversal across documents re-fetches (no bfcache).

use crate::{Browser, NavigateOpts, NavigationReason, PageEvent, PageId, SameDocumentKind};
use lp_dom::parse::DomRef;
use lp_dom::NodeId;
use lp_js::{ClassDef, JsClass, JsError, JsResult, NoParent, Scope, Value};
use lp_runtime::{Scheduler, TaskSource};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Instant;
use url::Url;

#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    url: String,
    /// `JSON.stringify` of the history state (`None` = null).
    state: Option<String>,
    /// Document sequence number: entries of the same document traverse
    /// without reloading.
    doc: u64,
}

struct NavState {
    url: String,
    entries: Vec<Entry>,
    index: usize,
    doc_seq: u64,
    scroll_restoration_manual: bool,
}

/// Per-page navigation state shared by the page and its windows.
pub struct PageNav {
    page: PageId,
    sched: Scheduler<Browser>,
    state: RefCell<NavState>,
    /// Current document in the page's DOM (keeps `document.URL` in sync).
    document: RefCell<Option<(DomRef, NodeId)>>,
    time_origin: Cell<Instant>,
}

impl PageNav {
    pub(crate) fn new(page: PageId, sched: Scheduler<Browser>) -> Rc<PageNav> {
        Rc::new(PageNav {
            page,
            sched,
            state: RefCell::new(NavState {
                url: "about:blank".into(),
                entries: Vec::new(),
                index: 0,
                doc_seq: 0,
                scroll_restoration_manual: false,
            }),
            document: RefCell::new(None),
            time_origin: Cell::new(Instant::now()),
        })
    }

    pub fn url(&self) -> String {
        self.state.borrow().url.clone()
    }

    pub fn history_length(&self) -> usize {
        self.state.borrow().entries.len().max(1)
    }

    /// Records a committed cross-document navigation.
    pub(crate) fn commit_document(&self, url: &str, replace: bool, document: Option<(DomRef, NodeId)>) {
        let mut st = self.state.borrow_mut();
        st.doc_seq += 1;
        let entry = Entry { url: url.to_string(), state: None, doc: st.doc_seq };
        if st.entries.is_empty() {
            st.entries.push(entry);
            st.index = 0;
        } else if replace {
            let i = st.index;
            st.entries[i] = entry;
        } else if traversing(&st, url) {
            // A history traversal committed: index was set by traverse().
            let i = st.index;
            st.entries[i].doc = st.doc_seq;
        } else {
            let i = st.index + 1;
            st.entries.truncate(i);
            st.entries.push(entry);
            st.index = i;
        }
        st.url = url.to_string();
        drop(st);
        *self.document.borrow_mut() = document;
        self.time_origin.set(Instant::now());
    }

    pub(crate) fn set_document(&self, document: Option<(DomRef, NodeId)>) {
        *self.document.borrow_mut() = document;
    }

    fn set_url_sync(&self, url: &str) {
        self.state.borrow_mut().url = url.to_string();
        if let Some((dom, doc)) = self.document.borrow().as_ref() {
            let _ = dom.borrow_mut().set_document_url(*doc, url);
        }
    }

    /// Same-document navigation (pushState/replaceState/fragment).
    fn same_document(&self, url: &str, state: Option<String>, kind: SameDocumentKind) {
        {
            let mut st = self.state.borrow_mut();
            let doc = st.entries.get(st.index).map_or(st.doc_seq, |e| e.doc);
            let entry = Entry { url: url.to_string(), state, doc };
            if st.entries.is_empty() {
                st.entries.push(entry);
            } else if kind == SameDocumentKind::Replace {
                let i = st.index;
                st.entries[i] = entry;
            } else {
                let i = st.index + 1;
                st.entries.truncate(i);
                st.entries.push(entry);
                st.index = i;
            }
        }
        self.set_url_sync(url);
        let page = self.page;
        let url = url.to_string();
        let document = self.state.borrow().doc_seq;
        self.sched.post(TaskSource::Navigation, move |b| {
            if b.page(page).is_some_and(|p| p.nav.state.borrow().doc_seq == document) {
                b.same_document_committed(page, url, kind);
            }
        });
    }

    fn navigate(&self, url: String, replace: bool) {
        let page = self.page;
        let document = self.state.borrow().doc_seq;
        self.sched.post(TaskSource::Navigation, move |b| {
            if !b.page(page).is_some_and(|p| p.nav.state.borrow().doc_seq == document) {
                return;
            }
            let opts = NavigateOpts { url, reason: NavigationReason::Script, cdp_request_id: None, referrer: None };
            if let Err(e) = b.navigate_with_history(page, opts, replace) {
                b.report_navigation_error(page, &e);
            }
        });
    }

    fn traverse(&self, delta: i64) {
        let target = {
            let st = self.state.borrow();
            // The JS number conversion saturates at i64's limits. Add in a
            // wider type so out-of-range deltas remain no-ops, not panics.
            let t = st.index as i128 + i128::from(delta);
            if delta == 0 || t < 0 || t >= st.entries.len() as i128 {
                None
            } else {
                Some((t as usize, st.entries[t as usize].clone(), st.doc_seq))
            }
        };
        let Some((index, entry, current_doc)) = target else {
            if delta == 0 {
                let url = self.url();
                self.navigate(url, true);
            }
            return;
        };
        // Traversal is queued as a task, like browsers do.
        let page = self.page;
        self.sched.post(TaskSource::Navigation, move |b| {
            let Some(nav) = b.page(page).map(|p| p.nav.clone()) else { return };
            {
                let mut state = nav.state.borrow_mut();
                // Navigation can replace the document, or pushState can replace
                // a forward entry, while this task waits. Never apply an obsolete
                // entry or write an index that is no longer in the history list.
                if state.doc_seq != current_doc || state.entries.get(index) != Some(&entry) {
                    return;
                }
                state.index = index;
            }
            if entry.doc == current_doc {
                nav.set_url_sync(&entry.url);
                // TODO(lp-webapi): fire popstate with the entry state (and hashchange).
                b.same_document_committed(page, entry.url.clone(), SameDocumentKind::Push);
            } else {
                let opts = NavigateOpts {
                    url: entry.url.clone(),
                    reason: NavigationReason::History,
                    cdp_request_id: None,
                    referrer: None,
                };
                if let Err(e) = b.navigate_with_history(page, opts, true) {
                    b.report_navigation_error(page, &e);
                }
            }
        });
    }

    fn state_json(&self) -> Option<String> {
        let st = self.state.borrow();
        st.entries.get(st.index).and_then(|e| e.state.clone())
    }

    /// Milliseconds since the current document's time origin.
    pub fn now_ms(&self) -> f64 {
        self.time_origin.get().elapsed().as_secs_f64() * 1000.0
    }
}

fn traversing(st: &NavState, url: &str) -> bool {
    st.entries.get(st.index).is_some_and(|e| e.url == url && e.doc != st.doc_seq)
}

impl Browser {
    pub(crate) fn same_document_committed(&mut self, page: PageId, url: String, kind: SameDocumentKind) {
        let Some(p) = self.page_mut(page) else { return };
        p.main_frame.url = url.clone();
        let frame_id = p.main_frame.id.clone();
        self.emit(page, PageEvent::FrameNavigatedWithinDocument { frame_id, url, kind });
    }

    fn report_navigation_error(&mut self, page: PageId, e: &JsError) {
        if let Some(realm) = self.page(page).and_then(|p| p.main_frame.main_realm().cloned()) {
            if let Ok(scope) = realm.scope() {
                scope.report_exception(e);
            }
        }
    }
}

fn parse(nav: &PageNav, input: Value<'_>) -> JsResult<Url> {
    let href = input.to_string()?;
    let base = Url::parse(&nav.url()).ok();
    Url::options()
        .base_url(base.as_ref())
        .parse(&href)
        .map_err(|_| JsError::dom("SyntaxError", format!("Failed to resolve URL '{href}'")))
}

fn same_document_except_fragment(a: &Url, b: &Url) -> bool {
    let mut a = a.clone();
    let mut b = b.clone();
    a.set_fragment(None);
    b.set_fragment(None);
    a == b
}

/// Location assignment: fragment-only changes are same-document.
fn assign(nav: &PageNav, input: Value<'_>, replace: bool) -> JsResult<()> {
    let target = parse(nav, input)?;
    let current = Url::parse(&nav.url()).ok();
    match current {
        Some(cur) if target.fragment().is_some() && same_document_except_fragment(&cur, &target) => {
            let kind = if replace { SameDocumentKind::Replace } else { SameDocumentKind::Fragment };
            nav.same_document(target.as_str(), None, kind);
            // TODO(lp-webapi): hashchange event.
        }
        _ => nav.navigate(target.to_string(), replace),
    }
    Ok(())
}

fn url_part(nav: &PageNav, f: impl Fn(&Url) -> String) -> String {
    Url::parse(&nav.url()).map(|u| f(&u)).unwrap_or_default()
}

pub struct Location;

impl JsClass for Location {
    const NAME: &'static str = "Location";
    type Data = Rc<PageNav>;
    type Parent = NoParent;

    fn define(def: &mut ClassDef<Self>) {
        def.accessor("href", |s, n| Ok(s.string(&n.url())), |_s, n, v| assign(n, v, false));
        def.method("toString", 0, |s, n, _a| Ok(s.string(&n.url())));
        def.method("assign", 1, |s, n, a| {
            a.require(1, "assign")?;
            assign(n, a.get(0), false)?;
            Ok(s.undefined())
        });
        def.method("replace", 1, |s, n, a| {
            a.require(1, "replace")?;
            assign(n, a.get(0), true)?;
            Ok(s.undefined())
        });
        def.method("reload", 0, |s, n, _a| {
            n.navigate(n.url(), true);
            Ok(s.undefined())
        });
        def.getter("origin", |s, n| {
            Ok(s.string(&url_part(n, |u| match u.origin() {
                o @ url::Origin::Tuple(..) => o.ascii_serialization(),
                url::Origin::Opaque(_) => "null".into(),
            })))
        });
        def.getter("protocol", |s, n| Ok(s.string(&url_part(n, |u| format!("{}:", u.scheme())))));
        def.getter("host", |s, n| {
            Ok(s.string(&url_part(n, |u| match (u.host_str(), u.port()) {
                (Some(h), Some(p)) => format!("{h}:{p}"),
                (Some(h), None) => h.to_string(),
                _ => String::new(),
            })))
        });
        def.getter("hostname", |s, n| Ok(s.string(&url_part(n, |u| u.host_str().unwrap_or("").to_string()))));
        def.getter("port", |s, n| Ok(s.string(&url_part(n, |u| u.port().map(|p| p.to_string()).unwrap_or_default()))));
        def.getter("pathname", |s, n| Ok(s.string(&url_part(n, |u| u.path().to_string()))));
        def.getter("search", |s, n| {
            Ok(s.string(&url_part(n, |u| match u.query() {
                Some(q) if !q.is_empty() => format!("?{q}"),
                _ => String::new(),
            })))
        });
        def.accessor(
            "hash",
            |s, n| {
                Ok(s.string(&url_part(n, |u| match u.fragment() {
                    Some(f) if !f.is_empty() => format!("#{f}"),
                    _ => String::new(),
                })))
            },
            |s, n, v| {
                let mut u = Url::parse(&n.url()).map_err(|_| JsError::Type("invalid current URL".into()))?;
                let h = v.to_string()?;
                u.set_fragment(Some(h.strip_prefix('#').unwrap_or(&h)));
                assign(n, s.string(u.as_str()), false)
            },
        );
    }
}

pub struct History;

impl JsClass for History {
    const NAME: &'static str = "History";
    type Data = Rc<PageNav>;
    type Parent = NoParent;

    fn define(def: &mut ClassDef<Self>) {
        def.getter("length", |s, n| Ok(s.number(n.history_length() as f64)));
        def.getter("state", |s, n| Ok(n.state_json().and_then(|j| s.parse_json(&j)).unwrap_or_else(|| s.null())));
        def.accessor(
            "scrollRestoration",
            |s, n| Ok(s.string(if n.state.borrow().scroll_restoration_manual { "manual" } else { "auto" })),
            |_s, n, v| {
                match v.to_string()?.as_str() {
                    "manual" => n.state.borrow_mut().scroll_restoration_manual = true,
                    "auto" => n.state.borrow_mut().scroll_restoration_manual = false,
                    _ => {} // invalid enum values are ignored
                }
                Ok(())
            },
        );
        def.method("pushState", 2, |s, n, a| change_state(s, n, a, SameDocumentKind::Push));
        def.method("replaceState", 2, |s, n, a| change_state(s, n, a, SameDocumentKind::Replace));
        def.method("back", 0, |s, n, _a| {
            n.traverse(-1);
            Ok(s.undefined())
        });
        def.method("forward", 0, |s, n, _a| {
            n.traverse(1);
            Ok(s.undefined())
        });
        def.method("go", 0, |s, n, a| {
            let d = a.get(0).to_number()?;
            n.traverse(if d.is_finite() { d.trunc() as i64 } else { 0 });
            Ok(s.undefined())
        });
    }
}

fn change_state<'s>(s: &Scope<'s>, n: &PageNav, a: lp_js::Args<'s>, kind: SameDocumentKind) -> JsResult<Value<'s>> {
    a.require(2, if kind == SameDocumentKind::Push { "pushState" } else { "replaceState" })?;
    // Approximates StructuredSerialize with JSON (functions/cycles throw).
    let state = match a.get(0) {
        v if v.is_nullish() => None,
        v => Some(v.to_json(0)?.ok_or_else(|| JsError::dom("DataCloneError", "state could not be cloned"))?),
    };
    let url = match a.opt(2).filter(|v| !v.is_nullish()) {
        Some(v) => {
            let target = parse(n, v)?;
            let current = Url::parse(&n.url()).ok();
            if current.as_ref().map(|c| c.origin()) != Some(target.origin()) {
                return Err(JsError::dom(
                    "SecurityError",
                    format!(
                        "A history state object with URL '{target}' cannot be created in a document with origin '{}'",
                        n.url()
                    ),
                ));
            }
            target.to_string()
        }
        None => n.url(),
    };
    n.same_document(&url, state, kind);
    Ok(s.undefined())
}

pub struct PerformanceData {
    nav: Rc<PageNav>,
    /// Wall-clock time of the time origin (ms since the Unix epoch).
    origin_wall_ms: f64,
}

impl PerformanceData {
    pub(crate) fn new(nav: Rc<PageNav>) -> PerformanceData {
        let now_wall = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs_f64() * 1000.0)
            .unwrap_or(0.0);
        let origin_wall_ms = now_wall - nav.now_ms();
        PerformanceData { nav, origin_wall_ms }
    }
}

pub struct Performance;

impl JsClass for Performance {
    const NAME: &'static str = "Performance";
    type Data = PerformanceData;
    type Parent = NoParent;

    fn define(def: &mut ClassDef<Self>) {
        def.method("now", 0, |s, p, _a| Ok(s.number(p.nav.now_ms())));
        def.getter("timeOrigin", |s, p| Ok(s.number(p.origin_wall_ms)));
        // User Timing is not implemented: entries are not recorded.
        def.method("mark", 1, |s, _p, _a| Ok(s.undefined()));
        def.method("measure", 1, |s, _p, _a| Ok(s.undefined()));
        def.method("clearMarks", 0, |s, _p, _a| Ok(s.undefined()));
        def.method("clearMeasures", 0, |s, _p, _a| Ok(s.undefined()));
        def.method("getEntries", 0, |s, _p, _a| Ok(s.array(&[])?.value()));
        def.method("getEntriesByName", 1, |s, _p, _a| Ok(s.array(&[])?.value()));
        def.method("getEntriesByType", 1, |s, _p, _a| Ok(s.array(&[])?.value()));
        def.method("toJSON", 0, |s, p, _a| {
            let o = s.object();
            o.set("timeOrigin", s.number(p.origin_wall_ms))?;
            Ok(o.value())
        });
    }
}
