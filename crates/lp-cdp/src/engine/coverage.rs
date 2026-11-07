//! Code coverage: `Profiler` precise coverage and the `Debugger` script
//! events it is read with (JavaScript), and `CSS` rule usage tracking.
//! What Playwright's `page.coverage` (and Puppeteer's) sends.
//!
//! JavaScript: JavaScriptCore's control flow profiler (patch 0019) counts
//! every basic block while some page has precise coverage on; it is per VM.
//! Turning it on or off recompiles all code; off, nothing is counted. While
//! it is on, every script the VM compiles is kept here with the page whose
//! main-world realm it first ran in, whether or not that page has coverage
//! on yet: a page starting coverage later gets the scripts it compiled since
//! (Chrome's isolate is the page's). Scripts are reported to a page's
//! `Debugger` sessions (`Debugger.scriptParsed`, sent from
//! [`EngineHost::flush`]), and `Profiler.takePreciseCoverage` returns V8's
//! block coverage: per function its whole range with how often it ran, and
//! inside it the blocks that never ran. Counts are the page's own: what ran
//! since it started coverage or since its last take (V8 resets them then).
//! As V8, a function is reported only when it ran since, or the reported
//! function around it did: coverage started after a script ran reports the
//! functions that ran again (with what they enclose), not its top level, and
//! leaves out a script none of whose code ran since, as Chrome does.
//!
//! CSS: a rule counts as used when one of its selectors matches an element of
//! its document while tracking is on. Documents are looked at when tracking
//! starts, before each style recalculation the page forces (a style or
//! layout query after a change, as Chrome marks rules when it recalculates),
//! after tasks that changed them (at most once per [`CSS_SAMPLE_INTERVAL`],
//! a frame) and when it stops; `@media` blocks count only while their query
//! matches.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use lp_browser::PageId;
use lp_js::{RealmId, ScriptCoverage};
use serde::Deserialize;
use serde_json::{json, Value};

use super::runtime::context_id;
use super::EngineHost;
use crate::msg::{CdpError, CdpResult};
use crate::target::{PageCommand, PageKey};

/// The least time between two looks at a document for used CSS rules after
/// tasks (a frame: Chrome recalculates styles at most once per frame unless
/// a query forces it, which samples too).
const CSS_SAMPLE_INTERVAL: Duration = Duration::from_millis(16);

#[derive(Default)]
pub(super) struct Coverage {
    pages: HashMap<PageKey, PageCoverage>,
    next_debugger_id: u64,
    /// While the VM counts: every page script it compiled since it was
    /// turned on, in that order, with the page it first ran in.
    scripts: Vec<Script>,
    /// The main-world realms of the pages seen while the VM counts, with
    /// page and frame: a script whose realm is gone by the time it is read
    /// is found among them.
    realms: HashMap<RealmId, (PageKey, String)>,
}

impl Coverage {
    /// Whether any page needs [`EngineHost::poll_coverage`].
    pub(super) fn active(&self) -> bool {
        self.pages.values().any(|p| !p.precise.is_empty() || !p.css.is_empty() || !p.tracking.is_empty())
    }
}

#[derive(Default)]
struct PageCoverage {
    /// Sessions with `Profiler.startPreciseCoverage` on: the VM counts while
    /// any page has one (one enable per page).
    precise: Vec<String>,
    /// Sessions with `Debugger.enable`.
    debugger: Vec<String>,
    main_frame: Option<String>,
    /// The page's scripts (indices into [`Coverage::scripts`]) in that order,
    /// by id, and how far into [`Coverage::scripts`] they were looked for.
    scripts: Vec<usize>,
    script_index: HashMap<isize, usize>,
    scripts_seen: usize,
    /// Per script, the block counts already reported or from before the page
    /// started coverage (the VM's counts go on), which the next take leaves out.
    baseline: HashMap<isize, HashMap<(i32, i32), u64>>,
    /// Sessions with `CSS.enable`, and with rule usage tracking on.
    css: Vec<String>,
    tracking: Vec<String>,
    /// Shared with the recalc hooks of the page's documents.
    sheets: Rc<RefCell<Sheets>>,
    /// When the documents were last looked at for used rules, and their DOM
    /// versions then.
    css_sampled: Option<(Instant, Vec<u64>)>,
}

#[derive(Default)]
struct Sheets {
    list: Vec<Sheet>,
    /// Rule usage tracking is on (the hooks sample only then).
    tracking: bool,
    /// The documents (`Document::page_dom_id`) whose style engine has a hook.
    hooked: Vec<u64>,
}

struct Script {
    id: isize,
    realm: RealmId,
    /// The page it first ran in (`None` once that page closed).
    page: Option<PageKey>,
    url: String,
    is_module: bool,
    /// The text as the page has it: the engine's module loader puts a line
    /// setting `import.meta.url` before a module's, which offsets leave out.
    source: String,
    /// UTF-16 length of that line (0 for a classic script).
    prefix: u32,
}

impl EngineHost {
    pub(super) fn profiler_domain(&mut self, action: &str, cmd: &PageCommand<'_, '_>) -> CdpResult<Option<Value>> {
        match action {
            "enable" => Ok(Some(json!({}))),
            "disable" => {
                self.stop_precise_coverage(cmd.page, cmd.session_id);
                Ok(Some(json!({})))
            }
            "startPreciseCoverage" => {
                if !lp_js::Engine::has_coverage() {
                    return Err(CdpError::server("Precise coverage needs the patched JavaScriptCore (block coverage)"));
                }
                let page = self.coverage.pages.entry(cmd.page).or_default();
                let first = page.precise.is_empty();
                if !page.precise.iter().any(|s| s == cmd.session_id) {
                    page.precise.push(cmd.session_id.to_owned());
                }
                if first {
                    let engine = &self.thread.browser.engine;
                    let counting = engine.coverage_enabled();
                    // What ran before is not this page's (another page has
                    // the VM counting: no recompile, the counts go on).
                    let baseline = if counting { baseline(&engine.take_coverage()) } else { HashMap::new() };
                    if let Err(e) = engine.set_coverage(true) {
                        if let Some(page) = self.coverage.pages.get_mut(&cmd.page) {
                            page.precise.retain(|s| s != cmd.session_id);
                        }
                        self.forget_idle_coverage(cmd.page);
                        return Err(CdpError::server(e.to_string()));
                    }
                    if !counting {
                        self.coverage.scripts.clear();
                        self.coverage.realms.clear();
                    }
                    let page = self.coverage.pages.get_mut(&cmd.page).expect("coverage page");
                    page.scripts.clear();
                    page.script_index.clear();
                    page.scripts_seen = 0;
                    page.baseline = baseline;
                    // The scripts the page compiled while another page had
                    // the VM counting are its own.
                    self.poll_scripts();
                }
                Ok(Some(json!({ "timestamp": timestamp() })))
            }
            "stopPreciseCoverage" => {
                self.stop_precise_coverage(cmd.page, cmd.session_id);
                Ok(Some(json!({})))
            }
            "takePreciseCoverage" => {
                self.poll_scripts();
                let result = self.take_precise_coverage(cmd.page);
                Ok(Some(json!({ "result": result, "timestamp": timestamp() })))
            }
            _ => Err(CdpError::unsupported(&cmd.command.method)),
        }
    }

    pub(super) fn debugger_domain(&mut self, action: &str, cmd: &PageCommand<'_, '_>) -> CdpResult<Option<Value>> {
        match action {
            "enable" => {
                self.coverage.next_debugger_id += 1;
                let id = format!("lp-debugger-{}", self.coverage.next_debugger_id);
                let page = self.coverage.pages.entry(cmd.page).or_default();
                if !page.debugger.iter().any(|s| s == cmd.session_id) {
                    page.debugger.push(cmd.session_id.to_owned());
                }
                // As Chrome: the scripts known so far, then each new one.
                self.poll_scripts();
                if let Some(page) = self.coverage.pages.get(&cmd.page) {
                    for &i in &page.scripts {
                        let params = self.script_parsed(&self.coverage.scripts[i], page);
                        self.event("Debugger.scriptParsed", &params, cmd.session_id);
                    }
                }
                Ok(Some(json!({ "debuggerId": id })))
            }
            "disable" => {
                if let Some(page) = self.coverage.pages.get_mut(&cmd.page) {
                    page.debugger.retain(|s| s != cmd.session_id);
                }
                self.forget_idle_coverage(cmd.page);
                Ok(Some(json!({})))
            }
            "getScriptSource" => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct P {
                    script_id: String,
                }
                let p: P = cmd.command.params()?;
                let source = p.script_id.parse::<isize>().ok().and_then(|id| {
                    let page = self.coverage.pages.get(&cmd.page)?;
                    page.script_index.get(&id).map(|&i| self.coverage.scripts[i].source.clone())
                });
                match source {
                    Some(source) => Ok(Some(json!({ "scriptSource": source }))),
                    None => Err(CdpError::server(format!("No script for id: {}", p.script_id))),
                }
            }
            // Nothing pauses: there are no breakpoints.
            "setSkipAllPauses"
            | "resume"
            | "setAsyncCallStackDepth"
            | "setBreakpointsActive"
            | "setPauseOnExceptions"
            | "setBlackboxPatterns" => Ok(Some(json!({}))),
            _ => Err(CdpError::unsupported(&cmd.command.method)),
        }
    }

    pub(super) fn css_domain(&mut self, action: &str, cmd: &PageCommand<'_, '_>) -> CdpResult<Option<Value>> {
        match action {
            "enable" => {
                let page = self.coverage.pages.entry(cmd.page).or_default();
                if !page.css.iter().any(|s| s == cmd.session_id) {
                    page.css.push(cmd.session_id.to_owned());
                }
                // As Chrome: the sheets so far, then each new one.
                self.poll_sheets(cmd.page, false);
                if let Some(page) = self.coverage.pages.get(&cmd.page) {
                    for sheet in page.sheets.borrow().list.iter().filter(|s| s.live) {
                        self.event("CSS.styleSheetAdded", &json!({ "header": sheet.header() }), cmd.session_id);
                    }
                }
                Ok(Some(json!({})))
            }
            "disable" => {
                if let Some(page) = self.coverage.pages.get_mut(&cmd.page) {
                    page.css.retain(|s| s != cmd.session_id);
                }
                self.forget_idle_coverage(cmd.page);
                Ok(Some(json!({})))
            }
            "getStyleSheetText" => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct P {
                    style_sheet_id: String,
                }
                let p: P = cmd.command.params()?;
                let text = self.coverage.pages.get(&cmd.page).and_then(|page| {
                    page.sheets.borrow().list.iter().find(|s| s.id == p.style_sheet_id).map(|s| s.text.clone())
                });
                match text {
                    Some(text) => Ok(Some(json!({ "text": text }))),
                    None => Err(CdpError::server(format!("No style sheet with given id found: {}", p.style_sheet_id))),
                }
            }
            "startRuleUsageTracking" => {
                let page = self.coverage.pages.entry(cmd.page).or_default();
                if page.tracking.is_empty() {
                    let mut sheets = page.sheets.borrow_mut();
                    for sheet in &mut sheets.list {
                        sheet.used.iter_mut().for_each(|u| *u = false);
                    }
                    sheets.tracking = true;
                    page.css_sampled = None;
                }
                if !page.tracking.iter().any(|s| s == cmd.session_id) {
                    page.tracking.push(cmd.session_id.to_owned());
                }
                self.poll_sheets(cmd.page, true);
                Ok(Some(json!({})))
            }
            "stopRuleUsageTracking" | "takeCoverageDelta" => {
                self.poll_sheets(cmd.page, true);
                let usage = self.rule_usage(cmd.page);
                if action == "stopRuleUsageTracking" {
                    self.stop_rule_usage_tracking(cmd.page, cmd.session_id);
                    Ok(Some(json!({ "ruleUsage": usage })))
                } else {
                    Ok(Some(json!({ "coverage": usage, "timestamp": timestamp() })))
                }
            }
            _ => Err(CdpError::unsupported(&cmd.command.method)),
        }
    }

    /// Ends `session`'s coverage and tracking on `page` (its detach).
    pub(super) fn coverage_session_detached(&mut self, page: PageKey, session: &str) {
        self.stop_precise_coverage(page, session);
        self.stop_rule_usage_tracking(page, session);
        if let Some(state) = self.coverage.pages.get_mut(&page) {
            state.debugger.retain(|s| s != session);
            state.css.retain(|s| s != session);
        }
        self.forget_idle_coverage(page);
    }

    /// `page` closed.
    pub(super) fn coverage_page_closed(&mut self, page: PageKey) {
        if let Some(state) = self.coverage.pages.remove(&page) {
            if !state.precise.is_empty() {
                let _ = self.thread.browser.engine.set_coverage(false);
            }
        }
        if !self.thread.browser.engine.coverage_enabled() {
            self.coverage.scripts.clear();
            self.coverage.realms.clear();
            return;
        }
        // Its scripts stay (other pages index them), without their text.
        for script in self.coverage.scripts.iter_mut().filter(|s| s.page == Some(page)) {
            script.page = None;
            script.source = String::new();
        }
        self.coverage.realms.retain(|_, (p, _)| *p != page);
    }

    fn stop_precise_coverage(&mut self, page: PageKey, session: &str) {
        let Some(state) = self.coverage.pages.get_mut(&page) else { return };
        let was_on = !state.precise.is_empty();
        state.precise.retain(|s| s != session);
        if was_on && state.precise.is_empty() {
            state.scripts.clear();
            state.script_index.clear();
            state.scripts_seen = 0;
            state.baseline.clear();
            let _ = self.thread.browser.engine.set_coverage(false);
            if !self.thread.browser.engine.coverage_enabled() {
                self.coverage.scripts.clear();
                self.coverage.realms.clear();
            }
        }
        self.forget_idle_coverage(page);
    }

    fn stop_rule_usage_tracking(&mut self, page: PageKey, session: &str) {
        let Some(state) = self.coverage.pages.get_mut(&page) else { return };
        let was_on = !state.tracking.is_empty();
        state.tracking.retain(|s| s != session);
        if was_on && state.tracking.is_empty() {
            let mut sheets = state.sheets.borrow_mut();
            sheets.tracking = false;
            sheets.hooked.clear();
            drop(sheets);
            // The hooks go from the documents still there (a gone one's
            // hook does nothing once its sheets are dropped or not tracking).
            if let Some(id) = self.pages.get(&page).map(|p| p.id) {
                for doc in page_documents(&self.thread.browser, id) {
                    doc.page_dom.layout().style().set_recalc_hook(None);
                }
            }
        }
        self.forget_idle_coverage(page);
    }

    /// Drops a page's state once no session uses any of it.
    fn forget_idle_coverage(&mut self, page: PageKey) {
        let idle = self.coverage.pages.get(&page).is_some_and(|p| {
            p.precise.is_empty() && p.debugger.is_empty() && p.css.is_empty() && p.tracking.is_empty()
        });
        if idle {
            self.coverage.pages.remove(&page);
        }
    }

    /// After engine work: new scripts and style sheets to report, documents
    /// to look at for used rules.
    pub(super) fn poll_coverage(&mut self) {
        if !self.coverage.active() {
            return;
        }
        self.poll_scripts();
        let pages: Vec<PageKey> = self
            .coverage
            .pages
            .iter()
            .filter(|(_, p)| !p.css.is_empty() || !p.tracking.is_empty())
            .map(|(k, _)| *k)
            .collect();
        for page in pages {
            self.poll_sheets(page, false);
        }
    }

    /// Records the main-world realms of every page, with their frames.
    fn note_coverage_realms(&mut self) {
        let pages: Vec<(PageKey, PageId)> = self.pages.iter().map(|(k, p)| (*k, p.id)).collect();
        for (key, id) in pages {
            for (realm, frame) in main_world_realms(&self.thread.browser, id) {
                self.coverage.realms.insert(realm, (key, frame));
            }
            let main_frame = self.thread.browser.page(id).map(|p| p.main_frame.id.0.clone());
            if let Some(state) = self.coverage.pages.get_mut(&key) {
                state.main_frame = main_frame;
            }
        }
    }

    /// Keeps the scripts the VM compiled since the last poll, each with the
    /// page it ran in, and hands each page with precise coverage the ones
    /// it does not have yet, reporting them to its `Debugger` sessions.
    fn poll_scripts(&mut self) {
        if !self.thread.browser.engine.coverage_enabled() {
            return;
        }
        self.note_coverage_realms();
        for script in self.thread.browser.engine.new_scripts() {
            let Some(realm) = script.realm else { continue };
            if !reported(&script) {
                continue;
            }
            // A live realm says its page and world; a gone one is found among
            // the realms the pages were seen with.
            let owner = match lp_js::Realm::by_id(realm) {
                Some(r) if !r.name().is_empty() => continue,
                Some(r) => super::realm_page(&r).and_then(|p| self.page_key(p)),
                None => None,
            };
            let Some(page) = owner.or_else(|| self.coverage.realms.get(&realm).map(|(p, _)| *p)) else { continue };
            let (prefix, source) = match script.source.split_once('\n') {
                Some((first, rest)) if script.is_module && first.starts_with("import.meta.url = ") => {
                    (first.encode_utf16().count() as u32 + 1, rest.to_owned())
                }
                _ => (0, script.source),
            };
            self.coverage.scripts.push(Script {
                id: script.source_id,
                realm,
                page: Some(page),
                url: script.url,
                is_module: script.is_module,
                source,
                prefix,
            });
        }
        let total = self.coverage.scripts.len();
        let keys: Vec<PageKey> = self
            .coverage
            .pages
            .iter()
            .filter(|(_, p)| !p.precise.is_empty() && p.scripts_seen < total)
            .map(|(k, _)| *k)
            .collect();
        for page in keys {
            let state = self.coverage.pages.get_mut(&page).expect("coverage page");
            let mut added = Vec::new();
            for i in state.scripts_seen..total {
                let script = &self.coverage.scripts[i];
                if script.page != Some(page) || state.script_index.contains_key(&script.id) {
                    continue;
                }
                state.script_index.insert(script.id, i);
                state.scripts.push(i);
                added.push(i);
            }
            state.scripts_seen = total;
            let state = &self.coverage.pages[&page];
            for i in added {
                let params = self.script_parsed(&self.coverage.scripts[i], state);
                for session in &state.debugger {
                    self.event("Debugger.scriptParsed", &params, session);
                }
            }
        }
    }

    /// `Profiler.takePreciseCoverage` for `page`: its scripts, V8's format,
    /// with what ran since it started coverage or since its last take.
    fn take_precise_coverage(&mut self, page: PageKey) -> Vec<Value> {
        let Some(state) = self.coverage.pages.get(&page) else { return Vec::new() };
        if state.precise.is_empty() {
            return Vec::new();
        }
        let mut result = Vec::new();
        let mut next_baseline = HashMap::new();
        for mut script in self.thread.browser.engine.take_coverage() {
            let Some(&index) = state.script_index.get(&script.source_id) else { continue };
            next_baseline.insert(script.source_id, block_counts(&script));
            if let Some(before) = state.baseline.get(&script.source_id) {
                since(&mut script, before);
            }
            let known = &self.coverage.scripts[index];
            unprefix(&mut script, known.prefix);
            let length = known.source.encode_utf16().count();
            let functions = v8_functions(&script, length, &known.source);
            // Nothing of it ran since coverage started: V8 leaves it out.
            if functions.is_empty() {
                continue;
            }
            result.push(json!({
                "scriptId": script.source_id.to_string(),
                "url": known.url,
                "functions": functions,
            }));
        }
        if let Some(state) = self.coverage.pages.get_mut(&page) {
            state.baseline = next_baseline;
        }
        result
    }

    /// Notes the style sheets of `page`'s documents (reporting new ones to
    /// `CSS` sessions) and, while tracking (`force`: also not yet due), the
    /// rules their documents use; hooks the documents' style recalculations
    /// while tracking.
    fn poll_sheets(&mut self, page: PageKey, force: bool) {
        let Some(id) = self.pages.get(&page).map(|p| p.id) else { return };
        let Some(state) = self.coverage.pages.get_mut(&page) else { return };
        let documents = page_documents(&self.thread.browser, id);
        let versions: Vec<u64> = documents.iter().map(|d| d.page_dom.dom.borrow().version()).collect();
        let tracking = !state.tracking.is_empty();
        let due = tracking
            && (force
                || match &state.css_sampled {
                    None => true,
                    Some((at, seen)) => *seen != versions && at.elapsed() >= CSS_SAMPLE_INTERVAL,
                });
        let shared = Rc::downgrade(&state.sheets);
        let mut sheets = state.sheets.borrow_mut();
        let mut added = Vec::new();
        for sheet in &mut sheets.list {
            sheet.live = false;
        }
        for doc in &documents {
            let dom = doc.page_dom.dom.borrow();
            let layout = doc.page_dom.layout();
            let style = layout.style();
            if tracking && (!sheets.hooked.contains(&doc.page_dom_id) || !style.has_recalc_hook()) {
                if !sheets.hooked.contains(&doc.page_dom_id) {
                    sheets.hooked.push(doc.page_dom_id);
                }
                style.set_recalc_hook(Some(recalc_hook(shared.clone(), doc.page_dom_id, doc.document)));
            }
            for owner in style.sheet_owners() {
                if !dom.is_alive(owner) || !dom.is_connected(owner).unwrap_or(false) {
                    continue;
                }
                let Some(revision) = style.sheet_revision(&dom, owner) else { continue };
                let key = (doc.page_dom_id, owner, revision);
                let index = match sheets.list.iter().position(|s| s.key == key) {
                    Some(i) => i,
                    None => {
                        let text = match style.sheet_text(owner) {
                            Some(text) => text.to_owned(),
                            None => dom.text_content(owner).ok().flatten().unwrap_or_default(),
                        };
                        // A `<link>` still loading (its text is a new sheet).
                        if text.is_empty() {
                            continue;
                        }
                        let link = dom
                            .node(owner)
                            .ok()
                            .and_then(|n| n.element())
                            .is_some_and(|e| e.is_html() && &*e.name.local == "link");
                        let url = if link {
                            dom.get_attribute(owner, "href")
                                .ok()
                                .flatten()
                                .and_then(|href| url::Url::parse(&doc.base_url).ok()?.join(href).ok())
                                .map(|u| u.to_string())
                                .unwrap_or_default()
                        } else {
                            doc.url.clone()
                        };
                        let rules = parse_rules(&text);
                        let used = vec![false; rules.len()];
                        let id = format!("{}.{}", doc.page_dom_id, sheets.list.len() + 1);
                        sheets.list.push(Sheet {
                            id,
                            key,
                            frame_id: doc.frame_id.clone(),
                            url,
                            inline: !link,
                            text,
                            rules,
                            used,
                            live: true,
                        });
                        added.push(sheets.list.len() - 1);
                        sheets.list.len() - 1
                    }
                };
                let sheet = &mut sheets.list[index];
                sheet.live = true;
                if due {
                    sheet.sample(&dom, doc.document, |query| style.match_media(query));
                }
            }
        }
        drop(sheets);
        if due {
            state.css_sampled = Some((Instant::now(), versions));
        }
        let state = &self.coverage.pages[&page];
        let sheets = state.sheets.borrow();
        for i in added {
            let header = sheets.list[i].header();
            for session in &state.css {
                self.event("CSS.styleSheetAdded", &json!({ "header": header }), session);
            }
        }
    }

    /// `CSS.stopRuleUsageTracking`'s `ruleUsage`: every rule of every sheet.
    fn rule_usage(&self, page: PageKey) -> Vec<Value> {
        let Some(state) = self.coverage.pages.get(&page) else { return Vec::new() };
        let mut usage = Vec::new();
        for sheet in &state.sheets.borrow().list {
            let units = Utf16Offsets::new(&sheet.text);
            for (rule, used) in sheet.rules.iter().zip(&sheet.used) {
                usage.push(json!({
                    "styleSheetId": sheet.id,
                    "startOffset": units.at(rule.start),
                    "endOffset": units.at(rule.end),
                    "used": used,
                }));
            }
        }
        usage
    }

    /// `Debugger.scriptParsed` for `script` of `page`.
    fn script_parsed(&self, script: &Script, page: &PageCoverage) -> Value {
        let frame_id = self.coverage.realms.get(&script.realm).map(|(_, f)| f.clone());
        script_parsed(script, frame_id.or_else(|| page.main_frame.clone()).unwrap_or_default())
    }
}

/// The hook of a document's style engine while its page tracks rule usage:
/// before a style recalculation, the rules of its sheets that match now.
fn recalc_hook(sheets: Weak<RefCell<Sheets>>, page_dom_id: u64, document: lp_dom::NodeId) -> lp_dom::style::RecalcHook {
    Rc::new(move |dom: &lp_dom::Dom, style: &lp_dom::style::StyleEngine| {
        let Some(sheets) = sheets.upgrade() else { return };
        // Busy: the host is looking at the sheets itself.
        let Ok(mut sheets) = sheets.try_borrow_mut() else { return };
        if !sheets.tracking {
            return;
        }
        for sheet in sheets.list.iter_mut().filter(|s| s.live && s.key.0 == page_dom_id) {
            sheet.sample(dom, document, |query| style.match_media(query));
        }
    })
}

/// The block counts of a script, by block.
fn block_counts(script: &ScriptCoverage) -> HashMap<(i32, i32), u64> {
    script.blocks.iter().map(|&(s, e, c)| ((s, e), c)).collect()
}

/// The block counts of every script.
fn baseline(scripts: &[ScriptCoverage]) -> HashMap<isize, HashMap<(i32, i32), u64>> {
    scripts.iter().map(|s| (s.source_id, block_counts(s))).collect()
}

/// `script` with only what ran since the counts `before`: a function ran
/// when its first block did.
fn since(script: &mut ScriptCoverage, before: &HashMap<(i32, i32), u64>) {
    for b in &mut script.blocks {
        b.2 = b.2.saturating_sub(before.get(&(b.0, b.1)).copied().unwrap_or(0));
    }
    for f in &mut script.functions {
        if !f.executed {
            continue;
        }
        let entry = script.blocks.iter().filter(|b| b.0 >= f.start as i32 && b.0 <= f.end as i32).min_by_key(|b| b.0);
        if let Some(entry) = entry {
            f.executed = entry.2 > 0;
        }
    }
}

/// Whether a script is the page's: internal scripts (helpers, init scripts,
/// the driver's) are not, nor anonymous ones evaluated through the API
/// (`Runtime.evaluate`, Playwright's); page code's `eval` and
/// `new Function` are, as anonymous scripts.
fn reported(script: &ScriptCoverage) -> bool {
    if script.url.is_empty() {
        page_url(&script.origin_url)
    } else {
        page_url(&script.url)
    }
}

/// The source names the engine evaluates its own scripts under
/// (`driver.rs`, `lp-webapi`'s `headers.rs` and `url_search_params.rs`).
const INTERNAL_SCRIPTS: &[&str] = &["lp-run", "lp-headers.js", "lp-url-search-params.js"];

/// Whether `url` names page code: any name (a relative `//# sourceURL=`
/// too) but the engine's own: the fixed names it evaluates its own code
/// under ([`INTERNAL_SCRIPTS`]), `init-script:N` and the internal schemes.
/// A page's own `//# sourceURL=lp-widget.js` is page code.
fn page_url(url: &str) -> bool {
    if url.is_empty() || url.starts_with("init-script:") || INTERNAL_SCRIPTS.contains(&url) {
        return false;
    }
    match url::Url::parse(url) {
        Ok(u) => !matches!(u.scheme(), "lightningpanda" | "lightpanda" | "init-script"),
        Err(_) => true,
    }
}

/// `Debugger.scriptParsed` for `script`, in the frame `frame_id`.
fn script_parsed(script: &Script, frame_id: String) -> Value {
    let (mut end_line, mut end_column) = (0usize, 0usize);
    for unit in script.source.encode_utf16() {
        if unit == u16::from(b'\n') {
            end_line += 1;
            end_column = 0;
        } else {
            end_column += 1;
        }
    }
    json!({
        "scriptId": script.id.to_string(),
        "url": script.url,
        "startLine": 0,
        "startColumn": 0,
        "endLine": end_line,
        "endColumn": end_column,
        "executionContextId": context_id(script.realm),
        "hash": "",
        "executionContextAuxData": { "isDefault": true, "type": "default", "frameId": frame_id },
        "isLiveEdit": false,
        "sourceMapURL": "",
        "hasSourceURL": false,
        "isModule": script.is_module,
        "length": script.source.encode_utf16().count(),
        "scriptLanguage": "JavaScript",
        "embedderName": script.url,
    })
}

fn timestamp() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64())
}

/// The main-world realms of `page`'s live frames, with their frame ids.
fn main_world_realms(browser: &lp_browser::Browser, page: PageId) -> Vec<(RealmId, String)> {
    let mut out = Vec::new();
    for frame in browser.frames(page) {
        browser.with_frame(page, &frame.id, |f| {
            for r in f.realms.iter().filter(|r| r.world.is_none()) {
                out.push((r.realm.id(), f.id.0.clone()));
            }
        });
    }
    out
}

/// A live document of a page, for its style sheets.
struct Document {
    page_dom: Rc<lp_webapi::PageDom>,
    page_dom_id: u64,
    document: lp_dom::NodeId,
    frame_id: String,
    url: String,
    base_url: String,
}

fn page_documents(browser: &lp_browser::Browser, page: PageId) -> Vec<Document> {
    let mut out = Vec::new();
    for frame in browser.frames(page) {
        browser.with_frame(page, &frame.id, |f| {
            let (Some(page_dom), Some(document)) = (f.page_dom.clone(), f.document) else { return };
            out.push(Document {
                page_dom_id: Rc::as_ptr(&page_dom) as usize as u64,
                page_dom,
                document,
                frame_id: f.id.0.clone(),
                url: f.url.clone(),
                base_url: f.base_url.clone(),
            });
        });
    }
    out
}

// ---------------------------------------------------------------------------
// V8's block coverage from the profiler's blocks

/// Offsets into the page's text of a script whose first `prefix` units the
/// engine added: blocks there go (they are its own top-level code).
fn unprefix(script: &mut ScriptCoverage, prefix: u32) {
    if prefix == 0 {
        return;
    }
    let p = prefix as i32;
    script.blocks.retain(|b| b.1 >= p);
    for b in &mut script.blocks {
        b.0 = (b.0 - p).max(0);
        b.1 -= p;
    }
    script.functions.retain(|f| f.start >= prefix);
    for f in &mut script.functions {
        f.start -= prefix;
        f.end -= prefix;
    }
}

/// The `functions` of a V8 `ScriptCoverage`: the script as a whole first, then
/// each function in source order. A function's first range is the function
/// with how often it ran; the ranges after it are the blocks in it (not in
/// nested functions) that never ran. Offsets are UTF-16, ends exclusive.
fn v8_functions(script: &ScriptCoverage, length: usize, source: &str) -> Vec<Value> {
    struct Function {
        start: u32,
        end: u32,
        executed: bool,
        name: String,
        entry: Option<(i32, u64)>,
        unexecuted: Vec<(u32, u32)>,
    }
    let length = length.max(script.blocks.iter().map(|b| b.1 as usize + 1).max().unwrap_or(0));
    let mut functions: Vec<Function> = script
        .functions
        .iter()
        .filter(|f| (f.end as usize) < length.max(1))
        .map(|f| Function {
            start: f.start,
            end: f.end,
            executed: f.executed,
            name: f.name.clone(),
            entry: None,
            unexecuted: Vec::new(),
        })
        .collect();
    functions.sort_by_key(|f| (f.start, std::cmp::Reverse(f.end)));
    functions.dedup_by_key(|f| (f.start, f.end));
    // JSC compiles a generator's or async function's body as a second
    // function of the same name, ending where the wrapper ends; V8 has one.
    // Fold it into the wrapper (its blocks then count for the wrapper).
    let mut folded = vec![false; functions.len()];
    for i in 0..functions.len() {
        let enclosing = (0..i)
            .rev()
            .find(|&j| !folded[j] && functions[j].start <= functions[i].start && functions[j].end >= functions[i].end);
        if let Some(j) = enclosing {
            if functions[j].end == functions[i].end && functions[j].name == functions[i].name {
                folded[i] = true;
                functions[j].executed |= functions[i].executed;
            }
        }
    }
    let mut index = 0;
    functions.retain(|_| {
        index += 1;
        !folded[index - 1]
    });
    // The script itself: index `functions.len()` below.
    let mut top_entry: Option<(i32, u64)> = None;
    let mut top_unexecuted: Vec<(u32, u32)> = Vec::new();

    let mut blocks: Vec<(i32, i32, u64)> = script.blocks.iter().copied().filter(|b| b.0 >= 0 && b.1 >= b.0).collect();
    blocks.sort();
    // Sweep: the functions open at a block's start, innermost last.
    let mut open: Vec<usize> = Vec::new();
    let mut next = 0;
    for (start, end, count) in blocks {
        let start_u = start as u32;
        while next < functions.len() && functions[next].start <= start_u {
            open.push(next);
            next += 1;
        }
        // Functions nest: once the innermost open one encloses the block,
        // all below it do.
        while open.last().is_some_and(|&f| functions[f].end < start_u) {
            open.pop();
        }
        let (entry, unexecuted, limit) = match open.last() {
            Some(&f) => {
                let func = &mut functions[f];
                (&mut func.entry, &mut func.unexecuted, func.end)
            }
            None => (&mut top_entry, &mut top_unexecuted, length.saturating_sub(1) as u32),
        };
        if entry.is_none_or(|(s, _)| start < s) {
            *entry = Some((start, count));
        }
        if count == 0 {
            let end_u = (end as u32).min(limit);
            match unexecuted.last_mut() {
                Some(last) if last.1 + 1 >= start_u => last.1 = last.1.max(end_u),
                _ => unexecuted.push((start_u, end_u)),
            }
        }
    }

    // A concise arrow (`() => expr`) that ran also ran its body expression,
    // though JSC leaves that block uncounted: drop the false 0 range.
    if !source.is_empty() {
        let text: Vec<u16> = source.encode_utf16().collect();
        for f in &mut functions {
            if !f.executed || f.unexecuted.is_empty() || f.end as usize >= text.len() {
                continue;
            }
            let own = String::from_utf16_lossy(&text[f.start as usize..=f.end as usize]);
            let Some(arrow) = own.find("=>") else { continue };
            let after = &own[arrow + 2..];
            let body = after.trim_start();
            if body.starts_with('{') {
                continue;
            }
            let body_start = f.start + own[..arrow + 2 + (after.len() - body.len())].encode_utf16().count() as u32;
            f.unexecuted.retain(|&(s, e)| !(s <= body_start + 1 && e >= f.end.saturating_sub(1)));
        }
        // JSC's block for the implicit return after a `return` covers only
        // the closing brace: nothing that could run, and V8 has no range there.
        let only_brace = |s: u32, e: u32| {
            (e as usize) < text.len()
                && String::from_utf16_lossy(&text[s as usize..=e as usize])
                    .trim_matches(|c: char| c.is_whitespace() || c == ';')
                    == "}"
        };
        for f in &mut functions {
            f.unexecuted.retain(|&(s, e)| !(e == f.end && only_brace(s, e)));
        }
    }

    // A class's implicit constructor comes with the range of JSC's own
    // source for it (`(function (...args) { super(...args); })`), not the
    // page's; V8 has no function for it. Every function in the source ends
    // with `}` or is an arrow.
    if !source.is_empty() {
        let text: Vec<u16> = source.encode_utf16().collect();
        functions.retain(|f| {
            let (s, e) = (f.start as usize, f.end as usize);
            e < text.len()
                && s <= e
                && (text[e] == u16::from(b'}') || String::from_utf16_lossy(&text[s..=e]).contains("=>"))
        });
    }

    // V8 names an anonymous function after what it is assigned to
    // (`ns.a = function () {}` is `ns.a`); JSC has only the function's own
    // name.
    if !source.is_empty() && functions.iter().any(|f| f.name.is_empty()) {
        let mut by_end: HashMap<u32, Vec<(u32, String)>> = HashMap::new();
        for ((start, end), name) in super::fn_names::inferred_names(source) {
            by_end.entry(end).or_default().push((start, name));
        }
        for f in functions.iter_mut().filter(|f| f.name.is_empty()) {
            if let Some((_, name)) = by_end.get(&f.end).and_then(|c| c.iter().min_by_key(|(s, _)| s.abs_diff(f.start)))
            {
                f.name = name.clone();
            }
        }
    }

    let ranges = |start: u32, end_inclusive: u32, count: u64, unexecuted: &[(u32, u32)]| {
        let mut ranges = vec![json!({ "startOffset": start, "endOffset": end_inclusive + 1, "count": count })];
        if count > 0 {
            for &(s, e) in unexecuted {
                ranges.push(json!({ "startOffset": s, "endOffset": e + 1, "count": 0 }));
            }
        }
        ranges
    };
    // A script whose top level ran before coverage was on (its code was not
    // compiled again, or its counts are from before) did not run since: 0,
    // as V8 resets the counts when precise coverage starts.
    let top_count = top_entry.map_or(0, |(_, count)| count);
    let top_end = length.saturating_sub(1) as u32;
    let mut candidates = vec![(0, top_end, top_count, "", &top_unexecuted)];
    for f in &functions {
        let count = match (f.executed, f.entry) {
            (false, _) => 0,
            (true, Some((_, count))) => count.max(1),
            (true, None) => 1,
        };
        candidates.push((f.start, f.end, count, f.name.as_str(), &f.unexecuted));
    }
    // V8 (`Coverage::CollectPrecise`) reports a function only when it ran, or
    // the function around it that is reported ran: code that ran before
    // coverage started and not since is left out, as in Chrome when
    // coverage starts after the page loaded. A script left with nothing is
    // left out ([`take_precise_coverage`]).
    let mut out = Vec::new();
    let mut nesting: Vec<(u32, u64)> = Vec::new();
    for (start, end, count, name, unexecuted) in candidates {
        while nesting.last().is_some_and(|&(parent_end, _)| parent_end < start) {
            nesting.pop();
        }
        let parent_ran = nesting.last().is_some_and(|&(_, c)| c > 0);
        if count == 0 && !parent_ran {
            continue;
        }
        nesting.push((end, count));
        out.push(json!({
            "functionName": name,
            "isBlockCoverage": true,
            "ranges": ranges(start, end, count, unexecuted),
        }));
    }
    out
}

// ---------------------------------------------------------------------------
// CSS rules

struct Sheet {
    id: String,
    /// (document, owner node, text revision): a sheet whose text changes is
    /// a new sheet, as its offsets are.
    key: (u64, lp_dom::NodeId, u64),
    frame_id: String,
    /// The `<link>`'s URL, or the document's for a `<style>`.
    url: String,
    inline: bool,
    text: String,
    rules: Vec<Rule>,
    used: Vec<bool>,
    /// Its owner is in its document now.
    live: bool,
}

impl Sheet {
    fn header(&self) -> Value {
        let (mut end_line, mut end_column) = (0usize, 0usize);
        for unit in self.text.encode_utf16() {
            if unit == u16::from(b'\n') {
                end_line += 1;
                end_column = 0;
            } else {
                end_column += 1;
            }
        }
        json!({
            "styleSheetId": self.id,
            "frameId": self.frame_id,
            "sourceURL": self.url,
            "origin": "regular",
            "title": "",
            "disabled": false,
            "isInline": self.inline,
            "isMutable": false,
            "isConstructed": false,
            "startLine": 0,
            "startColumn": 0,
            "length": self.text.encode_utf16().count(),
            "endLine": end_line,
            "endColumn": end_column,
        })
    }

    /// Marks the rules not used yet that match an element of `document` now.
    fn sample(&mut self, dom: &lp_dom::Dom, document: lp_dom::NodeId, media: impl Fn(&str) -> bool) {
        let mut media_cache: HashMap<&str, bool> = HashMap::new();
        for (rule, used) in self.rules.iter().zip(self.used.iter_mut()) {
            if *used {
                continue;
            }
            let applies = rule.media.iter().all(|q| *media_cache.entry(q.as_str()).or_insert_with(|| media(q)));
            if !applies {
                continue;
            }
            *used = rule.selectors.iter().any(|selector| {
                lp_dom::select::parse_query(selector)
                    .ok()
                    .and_then(|q| dom.query_first(document, &q).ok().flatten())
                    .is_some()
            });
        }
    }
}

/// A style rule: its byte range in the sheet (selector to closing brace),
/// its selectors made matchable (pseudo-elements dropped) and the `@media`
/// queries around it.
#[derive(Debug, Clone, PartialEq)]
struct Rule {
    start: usize,
    end: usize,
    selectors: Vec<String>,
    media: Vec<String>,
}

/// The style rules of a sheet, in `@media`, `@supports`, `@layer` and
/// `@container` blocks too (the latter two taken as applying). Other at-rules
/// (`@font-face`, `@keyframes`, ...) and rules nested in style rules are not
/// rules here.
fn parse_rules(text: &str) -> Vec<Rule> {
    let mut rules = Vec::new();
    let mut media = Vec::new();
    parse_block(text.as_bytes(), 0, text.len(), &mut media, &mut rules, text);
    rules
}

fn parse_block(b: &[u8], mut i: usize, end: usize, media: &mut Vec<String>, rules: &mut Vec<Rule>, text: &str) {
    loop {
        i = skip_space_and_comments(b, i, end);
        if i >= end {
            return;
        }
        if b[i] == b'}' || b[i] == b';' {
            i += 1;
            continue;
        }
        let start = i;
        // The prelude: up to `{` or `;` outside strings, comments and brackets.
        let Some((stop, at)) = scan_prelude(b, i, end) else { return };
        let prelude = text[start..stop].trim();
        if b.get(at).copied() != Some(b'{') {
            i = at + 1;
            continue;
        }
        let close = matching_brace(b, at, end);
        let body = (at + 1, close.min(end));
        if let Some(rest) = prelude.strip_prefix('@') {
            let name: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
                .collect::<String>()
                .to_ascii_lowercase();
            let condition = rest[name.len()..].trim();
            match name.as_str() {
                "media" => {
                    media.push(condition.to_owned());
                    parse_block(b, body.0, body.1, media, rules, text);
                    media.pop();
                }
                "supports" | "layer" | "container" | "document" | "-moz-document" | "scope" | "starting-style" => {
                    parse_block(b, body.0, body.1, media, rules, text)
                }
                _ => {}
            }
        } else {
            rules.push(Rule {
                start,
                end: (close + 1).min(end),
                selectors: split_selectors(prelude).into_iter().map(matchable).collect(),
                media: media.clone(),
            });
        }
        i = close + 1;
    }
}

fn skip_space_and_comments(b: &[u8], mut i: usize, end: usize) -> usize {
    while i < end {
        if b[i].is_ascii_whitespace() {
            i += 1;
        } else if b[i] == b'/' && b.get(i + 1) == Some(&b'*') {
            i = find_comment_end(b, i + 2, end);
        } else if b[i..end].starts_with(b"<!--") {
            i += 4;
        } else if b[i..end].starts_with(b"-->") {
            i += 3;
        } else {
            break;
        }
    }
    i
}

fn find_comment_end(b: &[u8], mut i: usize, end: usize) -> usize {
    while i + 1 < end {
        if b[i] == b'*' && b[i + 1] == b'/' {
            return i + 2;
        }
        i += 1;
    }
    end
}

fn skip_string(b: &[u8], mut i: usize, end: usize) -> usize {
    let quote = b[i];
    i += 1;
    while i < end {
        match b[i] {
            b'\\' => i += 2,
            c if c == quote => return i + 1,
            b'\n' => return i,
            _ => i += 1,
        }
    }
    end
}

/// The end of the prelude starting at `i` (trailing space excluded by the
/// caller) and the index of the `{` or `;` ending it.
fn scan_prelude(b: &[u8], mut i: usize, end: usize) -> Option<(usize, usize)> {
    let mut depth = 0usize;
    while i < end {
        match b[i] {
            b'"' | b'\'' => i = skip_string(b, i, end),
            b'/' if b.get(i + 1) == Some(&b'*') => i = find_comment_end(b, i + 2, end),
            b'\\' => i += 2,
            b'(' | b'[' => {
                depth += 1;
                i += 1;
            }
            b')' | b']' => {
                depth = depth.saturating_sub(1);
                i += 1;
            }
            b'{' | b';' if depth == 0 => return Some((i, i)),
            b'}' if depth == 0 => return Some((i, i)),
            _ => i += 1,
        }
    }
    None
}

/// The index of the `}` closing the block opened at `open` (or `end`).
fn matching_brace(b: &[u8], open: usize, end: usize) -> usize {
    let mut depth = 0usize;
    let mut i = open;
    while i < end {
        match b[i] {
            b'"' | b'\'' => {
                i = skip_string(b, i, end);
                continue;
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                i = find_comment_end(b, i + 2, end);
                continue;
            }
            b'\\' => {
                i += 2;
                continue;
            }
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return i;
                }
            }
            _ => {}
        }
        i += 1;
    }
    end
}

/// The selectors of a list, split at top-level commas.
fn split_selectors(list: &str) -> Vec<&str> {
    let b = list.as_bytes();
    let mut out = Vec::new();
    let (mut depth, mut start, mut i) = (0usize, 0usize, 0usize);
    while i < b.len() {
        match b[i] {
            b'"' | b'\'' => {
                i = skip_string(b, i, b.len());
                continue;
            }
            b'\\' => i += 1,
            b'(' | b'[' => depth += 1,
            b')' | b']' => depth = depth.saturating_sub(1),
            b',' if depth == 0 => {
                out.push(list[start..i].trim());
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    out.push(list[start..].trim());
    out.retain(|s| !s.is_empty());
    out
}

/// A selector `querySelector` can test: without pseudo-elements (a rule for
/// `a::before` is used when an `a` is there), `*` for an empty compound.
fn matchable(selector: &str) -> String {
    const LEGACY: [&str; 4] = [":before", ":after", ":first-line", ":first-letter"];
    let b = selector.as_bytes();
    let mut out = String::with_capacity(selector.len());
    let mut i = 0;
    while i < b.len() {
        let rest = &selector[i..];
        let pseudo_element = if rest.starts_with("::") {
            Some(2)
        } else {
            LEGACY
                .iter()
                .find(|p| {
                    rest.len() >= p.len()
                        && rest[..p.len()].eq_ignore_ascii_case(p)
                        && !rest[p.len()..].starts_with(|c: char| c.is_ascii_alphanumeric() || c == '-')
                })
                .map(|_| 1)
        };
        match (b[i], pseudo_element) {
            (b'"' | b'\'', _) => {
                let j = skip_string(b, i, b.len());
                out.push_str(&selector[i..j]);
                i = j;
            }
            (b'\\', _) => {
                let j = (i + 2).min(b.len());
                out.push_str(&selector[i..j]);
                i = j;
            }
            (b':', Some(colons)) => {
                // Skip the name and an argument list.
                let mut j = i + colons;
                while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'-' || b[j] == b'_') {
                    j += 1;
                }
                if b.get(j) == Some(&b'(') {
                    let mut depth = 0;
                    while j < b.len() {
                        match b[j] {
                            b'(' => depth += 1,
                            b')' => {
                                depth -= 1;
                                if depth == 0 {
                                    j += 1;
                                    break;
                                }
                            }
                            _ => {}
                        }
                        j += 1;
                    }
                }
                if out.is_empty() || out.ends_with(|c: char| c.is_whitespace() || matches!(c, '>' | '+' | '~')) {
                    out.push('*');
                }
                i = j;
            }
            _ => {
                let c = rest.chars().next().expect("non-empty");
                out.push(c);
                i += c.len_utf8();
            }
        }
    }
    let out = out.trim().to_owned();
    if out.is_empty() {
        "*".to_owned()
    } else {
        out
    }
}

/// UTF-16 offsets of byte offsets of a text.
struct Utf16Offsets<'a> {
    text: &'a str,
}

impl<'a> Utf16Offsets<'a> {
    fn new(text: &'a str) -> Self {
        Utf16Offsets { text }
    }
    fn at(&self, byte: usize) -> usize {
        let byte = byte.min(self.text.len());
        if self.text.is_ascii() {
            return byte;
        }
        self.text[..byte].encode_utf16().count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lp_js::FunctionCoverage;

    fn script(blocks: Vec<(i32, i32, u64)>, functions: Vec<(u32, u32, bool, &str)>) -> ScriptCoverage {
        ScriptCoverage {
            source_id: 1,
            realm: None,
            url: "https://a.test/x.js".into(),
            origin_url: String::new(),
            is_module: false,
            source: String::new(),
            blocks,
            functions: functions
                .into_iter()
                .map(|(start, end, executed, name)| FunctionCoverage { start, end, executed, name: name.into() })
                .collect(),
        }
    }

    #[test]
    fn v8_shape_nests_unexecuted_blocks_in_their_function() {
        // 0..100 script; f = 10..50 ran twice with an untaken branch 30..39;
        // g = 60..90 never ran.
        let s = script(
            vec![(0, 9, 1), (51, 59, 1), (91, 99, 1), (10, 29, 2), (30, 39, 0), (40, 50, 2), (60, 90, 0)],
            vec![(10, 50, true, "f"), (60, 90, false, "g")],
        );
        let f = v8_functions(&s, 100, "");
        assert_eq!(f.len(), 3);
        assert_eq!(f[0]["ranges"], json!([{ "startOffset": 0, "endOffset": 100, "count": 1 }]));
        assert_eq!(f[1]["functionName"], "f");
        assert_eq!(
            f[1]["ranges"],
            json!([{ "startOffset": 10, "endOffset": 51, "count": 2 }, { "startOffset": 30, "endOffset": 40, "count": 0 }])
        );
        assert_eq!(f[2]["ranges"], json!([{ "startOffset": 60, "endOffset": 91, "count": 0 }]));
    }

    #[test]
    fn v8_shape_folds_generator_bodies_and_counts_concise_arrow_bodies() {
        // `function* g(){}` and its body function 13..15 (JSC's second
        // function of the same name): one entry, as V8.
        let src = "function* g(){} const a = () => 7; a(); g();";
        let s = script(
            vec![(0, 43, 1), (0, 14, 1), (13, 14, 1), (26, 32, 1), (32, 32, 0)],
            vec![(0, 14, true, "g"), (13, 14, true, "g"), (26, 32, true, "a")],
        );
        let f = v8_functions(&s, src.encode_utf16().count(), src);
        let names: Vec<&str> = f.iter().map(|f| f["functionName"].as_str().unwrap()).collect();
        assert_eq!(names.iter().filter(|n| **n == "g").count(), 1, "{f:?}");
        // The arrow ran, so its body did: no count-0 range for `7`.
        assert_eq!(f.last().unwrap()["ranges"], json!([{ "startOffset": 26, "endOffset": 33, "count": 1 }]));
    }

    #[test]
    fn v8_shape_leaves_out_what_did_not_run_since_coverage_started() {
        // The script ran before coverage started (no top-level blocks); f =
        // 10..50 ran since and declares h = 20..30 (never ran); g = 60..90
        // ran before only; k = 70..80 inside g never ran.
        let s = script(
            vec![(10, 19, 1), (31, 50, 1), (20, 30, 0)],
            vec![(10, 50, true, "f"), (20, 30, false, "h"), (60, 90, false, "g"), (70, 80, false, "k")],
        );
        let f = v8_functions(&s, 100, "");
        let names: Vec<&str> = f.iter().map(|f| f["functionName"].as_str().unwrap()).collect();
        assert_eq!(names, ["f", "h"]);
        assert_eq!(f[0]["ranges"], json!([{ "startOffset": 10, "endOffset": 51, "count": 1 }]));
        assert_eq!(f[1]["ranges"], json!([{ "startOffset": 20, "endOffset": 31, "count": 0 }]));
        // Nothing ran since: nothing to report.
        let idle = script(vec![(0, 99, 0)], vec![(10, 50, false, "f")]);
        assert!(v8_functions(&idle, 100, "").is_empty());
    }

    #[test]
    fn v8_shape_names_anonymous_functions_as_v8_and_drops_implicit_constructors() {
        let src = "ns.a = function () { return 1; }; class L extends K {}";
        let f_end = src.find("};").unwrap() as u32;
        // JSC's implicit constructor of L: a range in its own source.
        let s = script(vec![(0, 51, 1)], vec![(7, f_end, true, ""), (1, 39, false, "L")]);
        let f = v8_functions(&s, src.len(), src);
        let names: Vec<&str> = f.iter().map(|f| f["functionName"].as_str().unwrap()).collect();
        assert_eq!(names, ["", "ns.a"], "{f:?}");
    }

    #[test]
    fn page_scripts_by_url() {
        for url in [
            "https://a.test/x.js",
            "nicename.js",
            "lp-widget.js",
            "lp-run.js",
            "dir/x.js",
            "data:text/html,x",
            "file:///x.js",
        ] {
            assert!(page_url(url), "{url}");
        }
        for url in [
            "",
            "lp-run",
            "lp-headers.js",
            "lp-url-search-params.js",
            "init-script:3",
            "lightningpanda://internal/clock.js",
            "lightpanda://internal/helpers.js",
        ] {
            assert!(!page_url(url), "{url}");
        }
    }

    #[test]
    fn counts_since_a_baseline() {
        let mut s = script(vec![(0, 9, 3), (10, 20, 2), (30, 40, 1)], vec![(10, 20, true, "f"), (30, 40, true, "g")]);
        let before = HashMap::from([((0, 9), 1), ((10, 20), 2)]);
        since(&mut s, &before);
        assert_eq!(s.blocks, vec![(0, 9, 2), (10, 20, 0), (30, 40, 1)]);
        assert!(!s.functions[0].executed);
        assert!(s.functions[1].executed);
    }

    #[test]
    fn css_rules_with_offsets_media_and_pseudo_elements() {
        let text = "/* c */ a, b::before { x: '}' }\n@media (max-width: 1px) { .m { y: 1 } }\n@font-face { src: url(x) }\np:hover > ::selection{}";
        let rules = parse_rules(text);
        assert_eq!(rules.len(), 3);
        assert_eq!(&text[rules[0].start..rules[0].end], "a, b::before { x: '}' }");
        assert_eq!(rules[0].selectors, vec!["a", "b"]);
        assert_eq!(rules[1].media, vec!["(max-width: 1px)"]);
        assert_eq!(&text[rules[1].start..rules[1].end], ".m { y: 1 }");
        assert_eq!(rules[2].selectors, vec!["p:hover > *"]);
    }
}
