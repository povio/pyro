//! `navigator.userAgentData`, `navigator.permissions`, `navigator.clipboard`,
//! `navigator.geolocation`, `ClipboardItem` and `Notification`. Their classes are made in JS (navigator_extras.js) on
//! a realm's first use of any of them, so realms that never touch them pay
//! nothing. Permissions and the clipboard belong to the browser context
//! ([`ContextServices`], shared by its pages); client hints come from the
//! page's user agent override.
use std::cell::RefCell;
use std::fmt::Write as _;
use std::rc::Rc;

use pyro_js::{JsError, JsResult, Persistent, Scope, Value};
use rustc_hash::FxHashMap;

use crate::page::PageDom;

/// `Emulation.setUserAgentOverride`'s `userAgentMetadata` (client hints).
/// Optional fields left out (`None`) take the browser's own values, as
/// Chrome fills them in (Playwright's `userAgent` option sends no brands).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UserAgentMetadata {
    /// (brand, version) pairs.
    pub brands: Option<Vec<(String, String)>>,
    pub full_version_list: Option<Vec<(String, String)>>,
    pub full_version: Option<String>,
    pub platform: String,
    pub platform_version: String,
    pub architecture: String,
    pub model: String,
    pub mobile: bool,
    pub bitness: Option<String>,
    pub wow64: bool,
    pub form_factors: Option<Vec<String>>,
}

/// Chromium's brand list for a major version
/// (`embedder_support::GenerateBrandVersionList`): a GREASE brand whose name,
/// version and position derive from the major version, then Chromium and
/// Google Chrome. Measured: Chrome 156 reports
/// `"Not:A-Brand";v="8", "Chromium";v="156", "Google Chrome";v="156"`.
fn brand_list(seed: usize, version: &str, full: bool) -> Vec<(String, String)> {
    const CHARS: [char; 11] = [' ', '(', ':', '-', '.', '/', ')', ';', '=', '?', '_'];
    const VERSIONS: [&str; 3] = ["8", "99", "24"];
    const ORDERS: [[usize; 3]; 6] = [[0, 1, 2], [0, 2, 1], [1, 0, 2], [1, 2, 0], [2, 0, 1], [2, 1, 0]];
    let name = format!("Not{}A{}Brand", CHARS[seed % CHARS.len()], CHARS[(seed + 1) % CHARS.len()]);
    let grease = VERSIONS[seed % VERSIONS.len()];
    let grease = if full { format!("{grease}.0.0.0") } else { grease.to_owned() };
    let order = ORDERS[seed % ORDERS.len()];
    let mut list = vec![Default::default(); 3];
    list[order[0]] = (name, grease);
    list[order[1]] = ("Chromium".to_owned(), version.to_owned());
    list[order[2]] = ("Google Chrome".to_owned(), version.to_owned());
    list
}

impl UserAgentMetadata {
    /// The browser's own hints for its user agent string: Chrome's brands
    /// when it names a Chrome version, none otherwise (pyro's
    /// default agent names none).
    pub fn from_user_agent(ua: &str, platform: &str) -> UserAgentMetadata {
        let full = ua
            .split_whitespace()
            .find_map(|t| t.strip_prefix("Chrome/").or_else(|| t.strip_prefix("HeadlessChrome/")))
            .unwrap_or("");
        let major = full.split('.').next().unwrap_or("");
        let seed = major.parse::<usize>().ok();
        let mobile = ua.contains("Mobile");
        let platform = match platform {
            p if p.starts_with("Mac") => "macOS",
            p if p.starts_with("Win") => "Windows",
            p if p.starts_with("Linux") && ua.contains("Android") => "Android",
            p if p.starts_with("Linux") => "Linux",
            p if p.starts_with("iP") => "iOS",
            _ => "",
        };
        UserAgentMetadata {
            brands: Some(seed.map(|s| brand_list(s, major, false)).unwrap_or_default()),
            full_version_list: Some(seed.map(|s| brand_list(s, full, true)).unwrap_or_default()),
            full_version: Some(full.into()),
            platform: platform.into(),
            platform_version: String::new(),
            architecture: if cfg!(target_arch = "aarch64") { "arm".into() } else { "x86".into() },
            model: String::new(),
            mobile,
            bitness: Some("64".into()),
            wow64: false,
            form_factors: Some(vec![if mobile { "Mobile".into() } else { "Desktop".into() }]),
        }
    }

    /// Chrome's hints for a user agent overridden without metadata: all
    /// empty, so pages do not take the agent for Chrome's.
    pub fn empty() -> UserAgentMetadata {
        UserAgentMetadata {
            brands: Some(Vec::new()),
            full_version_list: Some(Vec::new()),
            full_version: Some(String::new()),
            bitness: Some(String::new()),
            form_factors: Some(Vec::new()),
            ..Default::default()
        }
    }

    /// The JSON navigator_extras.js reads; `defaults` fills the fields the
    /// metadata left out.
    fn json(&self, defaults: impl FnOnce() -> UserAgentMetadata) -> String {
        let complete = self.brands.is_some()
            && self.full_version_list.is_some()
            && self.full_version.is_some()
            && self.bitness.is_some()
            && self.form_factors.is_some();
        let defaults = if complete { None } else { Some(defaults()) };
        let list = |own: &Option<Vec<(String, String)>>,
                    of: fn(&UserAgentMetadata) -> &Option<Vec<(String, String)>>| {
            own.clone().or_else(|| defaults.as_ref().and_then(|d| of(d).clone())).unwrap_or_default()
        };
        let brand_list = list(&self.brands, |d| &d.brands);
        let full_version_list = list(&self.full_version_list, |d| &d.full_version_list);
        let pick = |own: &Option<String>, of: fn(&UserAgentMetadata) -> &Option<String>| {
            own.clone().or_else(|| defaults.as_ref().and_then(|d| of(d).clone())).unwrap_or_default()
        };
        let full_version = pick(&self.full_version, |d| &d.full_version);
        let bitness = pick(&self.bitness, |d| &d.bitness);
        let form_factors = self
            .form_factors
            .clone()
            .or_else(|| defaults.as_ref().and_then(|d| d.form_factors.clone()))
            .unwrap_or_default();
        let mut out = String::from("{");
        let brands = |out: &mut String, key: &str, list: &[(String, String)]| {
            let _ = write!(out, "\"{key}\":[");
            for (i, (brand, version)) in list.iter().enumerate() {
                let sep = if i == 0 { "" } else { "," };
                let _ = write!(out, "{sep}{{\"brand\":{},\"version\":{}}}", quote(brand), quote(version));
            }
            out.push_str("],");
        };
        brands(&mut out, "brands", &brand_list);
        brands(&mut out, "fullVersionList", &full_version_list);
        let factors: Vec<String> = form_factors.iter().map(|f| quote(f)).collect();
        let _ = write!(
            out,
            "\"mobile\":{},\"platform\":{},\"platformVersion\":{},\"architecture\":{},\"model\":{},\"bitness\":{},\"wow64\":{},\"uaFullVersion\":{},\"formFactors\":[{}]}}",
            self.mobile,
            quote(&self.platform),
            quote(&self.platform_version),
            quote(&self.architecture),
            quote(&self.model),
            quote(&bitness),
            self.wow64,
            quote(&full_version),
            factors.join(",")
        );
        out
    }
}

/// A JSON string literal.
pub(crate) fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PermissionSetting {
    Granted,
    Denied,
    Prompt,
}

impl PermissionSetting {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Granted => "granted",
            Self::Denied => "denied",
            Self::Prompt => "prompt",
        }
    }
}

/// The settings of one origin (or of every origin, keyed `None`).
#[derive(Default)]
struct OriginPermissions {
    settings: FxHashMap<String, PermissionSetting>,
    /// `Browser.grantPermissions` denies what it does not list.
    deny_others: bool,
}

/// One clipboard entry: MIME type, data, and whether the data is bytes
/// (each `char` one byte) rather than text.
type ClipboardEntry = (String, String, bool);

/// State a browser context's pages share: its permission table and its
/// clipboard (in memory; nothing reaches the system clipboard).
#[derive(Default)]
pub struct ContextServices {
    permissions: RefCell<FxHashMap<Option<String>, OriginPermissions>>,
    clipboard: RefCell<Vec<ClipboardEntry>>,
    /// Documents with PermissionStatus change listeners.
    watchers: RefCell<Vec<std::rc::Weak<PageDom>>>,
    /// `Emulation.setGeolocationOverride`. Outer `None`: no override.
    geolocation: RefCell<Option<Option<GeolocationFix>>>,
    /// Service worker registrations and clients (`crate::service_worker`).
    pub(crate) service_workers: crate::service_worker::Registry,
}

/// A position `navigator.geolocation` reports. CDP's optional coordinates are
/// `None` when the override leaves them out.
#[derive(Clone, Debug, PartialEq)]
pub struct GeolocationFix {
    pub latitude: f64,
    pub longitude: f64,
    pub accuracy: f64,
    pub altitude: Option<f64>,
    pub altitude_accuracy: Option<f64>,
    pub heading: Option<f64>,
    pub speed: Option<f64>,
}

impl ContextServices {
    /// Whether loads of this context's frames may go through a service
    /// worker (it has registrations): the one test a load pays otherwise.
    pub fn may_route_loads(&self) -> bool {
        self.service_workers.has_registrations()
    }

    /// `Browser.grantPermissions`: `names` (permission names as
    /// `navigator.permissions.query` takes them) become granted for
    /// `origin` (`None`: every origin) and the rest denied.
    pub fn grant(&self, origin: Option<String>, names: &[String]) {
        let mut table = self.permissions.borrow_mut();
        let entry = table.entry(origin).or_default();
        entry.settings = names.iter().map(|n| (n.clone(), PermissionSetting::Granted)).collect();
        entry.deny_others = true;
    }

    /// `Browser.setPermission`: one permission's setting for `origin`.
    pub fn set(&self, origin: Option<String>, name: &str, setting: PermissionSetting) {
        self.permissions.borrow_mut().entry(origin).or_default().settings.insert(name.to_owned(), setting);
    }

    /// `Browser.resetPermissions`.
    pub fn reset(&self) {
        self.permissions.borrow_mut().clear();
    }

    /// The state `navigator.permissions.query({name})` reports in a document
    /// of `origin` (`None`: an opaque origin).
    pub fn state(&self, origin: Option<&str>, name: &str) -> PermissionSetting {
        let table = self.permissions.borrow();
        // The origin's own entry first, then the every-origin one.
        let own = origin.and_then(|o| table.get(&Some(o.to_owned())));
        for entry in own.into_iter().chain(table.get(&None)) {
            if let Some(setting) = entry.settings.get(name) {
                return *setting;
            }
            if entry.deny_others {
                return PermissionSetting::Denied;
            }
        }
        // Chrome grants writing to the clipboard of a focused page.
        if name == "clipboard-write" {
            PermissionSetting::Granted
        } else {
            PermissionSetting::Prompt
        }
    }

    /// After a permission change: each document with PermissionStatus
    /// change listeners re-reads their states in one task, firing `change`
    /// on those that flipped. Documents without listeners cost nothing.
    pub fn notify_changed(&self) {
        let mut watchers = self.watchers.borrow_mut();
        watchers.retain(|w| w.upgrade().is_some_and(|p| !p.is_disposed()));
        let pages: Vec<Rc<PageDom>> = watchers.iter().filter_map(std::rc::Weak::upgrade).collect();
        drop(watchers);
        for page in pages {
            let weak = Rc::downgrade(&page);
            page.post_task(Box::new(move || {
                let Some(page) = weak.upgrade() else { return };
                let watchers: Vec<_> = page.permission_watchers.borrow().clone();
                for (realm, refresh) in watchers {
                    let Some(realm) = realm.upgrade() else { continue };
                    let Ok(s) = realm.scope() else { continue };
                    if let Some(f) = refresh.get(&s).as_object() {
                        if let Err(e) = f.call(None, &[]) {
                            s.report_exception(&e);
                        }
                    }
                }
            }));
        }
    }

    fn watch(&self, page: &Rc<PageDom>) {
        let mut watchers = self.watchers.borrow_mut();
        if !watchers.iter().any(|w| std::ptr::eq(w.as_ptr(), Rc::as_ptr(page))) {
            watchers.push(Rc::downgrade(page));
        }
    }

    /// `Emulation.setGeolocationOverride`: `Some(None)` makes the position
    /// unavailable, `None` clears the override.
    pub fn set_geolocation(&self, fix: Option<Option<GeolocationFix>>) {
        *self.geolocation.borrow_mut() = fix;
    }

    /// The clipboard's `text/plain` entry (CDP-side inspection and tests).
    pub fn clipboard_text(&self) -> Option<String> {
        self.clipboard.borrow().iter().find(|e| e.0 == "text/plain").map(|e| e.1.clone())
    }

    /// The clipboard's text entries (MIME type, data), as a paste reads them.
    pub(crate) fn clipboard_strings(&self) -> Vec<(String, String)> {
        self.clipboard.borrow().iter().filter(|e| !e.2).map(|e| (e.0.clone(), e.1.clone())).collect()
    }

    /// Replaces the clipboard with text entries (copy, cut).
    pub(crate) fn write_clipboard_strings(&self, items: Vec<(String, String)>) {
        *self.clipboard.borrow_mut() = items.into_iter().map(|(ty, data)| (ty, data, false)).collect();
    }
}

/// Member `name` (`userAgentData`, `permissions`, `clipboard`, `geolocation` or
/// `ClipboardItem`) of this realm's extras, made on first use.
/// They live on the realm's `navigator` (a hidden property), which is
/// itself kept per realm.
pub(crate) fn member<'s>(s: &Scope<'s>, page: &Rc<PageDom>, name: &str) -> JsResult<Value<'s>> {
    const KEY: &str = "lpNavigatorExtras";
    let unavailable = || JsError::Type("navigator extras are unavailable".into());
    let navigator = crate::window::navigator(s, page)?.as_object().ok_or_else(unavailable)?;
    let extras = match s.get_hidden(navigator, KEY)?.and_then(|v| v.as_object()) {
        Some(extras) => extras,
        None => {
            let extras = make(s)?;
            s.set_hidden(navigator, KEY, extras.value())?;
            extras
        }
    };
    extras.get(name)
}

fn make<'s>(s: &Scope<'s>) -> JsResult<pyro_js::Object<'s>> {
    let native = s.object();
    let ua = s.function("uaData", |s, _, _| {
        let page = crate::target::page_of(s)?;
        Ok(s.string(&user_agent_json(&page)))
    })?;
    native.set_atom("uaData", ua.value())?;
    let state = s.function("permissionState", |s, _, args| {
        let page = crate::target::page_of(s)?;
        let name = args.get(0).to_string()?;
        Ok(s.string(permission_state(&page, &name).as_str()))
    })?;
    native.set_atom("permissionState", state.value())?;
    let watch = s.function("watchPermissions", |s, _, args| {
        let page = crate::target::page_of(s)?;
        let refresh = args.get(0);
        page.permission_watchers.borrow_mut().push((s.realm().downgrade(), Persistent::new(s, refresh)));
        page.context_services().watch(&page);
        Ok(s.undefined())
    })?;
    native.set_atom("watchPermissions", watch.value())?;
    let read = s.function("clipboardRead", |s, _, _| {
        let page = crate::target::page_of(s)?;
        if permission_state(&page, "clipboard-read") != PermissionSetting::Granted {
            return Ok(s.null());
        }
        let services = page.context_services();
        let entries = services.clipboard.borrow();
        let mut out = String::from("[");
        for (i, (ty, data, binary)) in entries.iter().enumerate() {
            let sep = if i == 0 { "" } else { "," };
            let _ = write!(out, "{sep}[{},{},{binary}]", quote(ty), quote(data));
        }
        out.push(']');
        Ok(s.string(&out))
    })?;
    native.set_atom("clipboardRead", read.value())?;
    let writable = s.function("clipboardWritable", |s, _, _| {
        let page = crate::target::page_of(s)?;
        Ok(s.boolean(permission_state(&page, "clipboard-write") != PermissionSetting::Denied))
    })?;
    native.set_atom("clipboardWritable", writable.value())?;
    let write = s.function("clipboardWrite", |s, _, args| {
        let page = crate::target::page_of(s)?;
        let parsed = s.parse_json(&args.get(0).to_string()?).and_then(|v| v.as_object());
        let parsed = parsed.ok_or_else(|| JsError::Type("invalid clipboard data".into()))?;
        let mut entries = Vec::new();
        for i in 0..parsed.get_atom("length")?.to_number()? as u32 {
            let entry =
                parsed.get_index(i)?.as_object().ok_or_else(|| JsError::Type("invalid clipboard data".into()))?;
            entries.push((
                entry.get_index(0)?.to_string()?,
                entry.get_index(1)?.to_string()?,
                entry.get_index(2)?.to_bool(),
            ));
        }
        *page.context_services().clipboard.borrow_mut() = entries;
        Ok(s.undefined())
    })?;
    native.set_atom("clipboardWrite", write.value())?;
    let position = s.function("geolocationPosition", |s, _, _| {
        let page = crate::target::page_of(s)?;
        let services = page.context_services();
        let fix = services.geolocation.borrow();
        let Some(Some(fix)) = fix.as_ref() else { return Ok(s.null()) };
        let result = s.object();
        result.set_atom("latitude", s.number(fix.latitude))?;
        result.set_atom("longitude", s.number(fix.longitude))?;
        result.set_atom("accuracy", s.number(fix.accuracy))?;
        let optional = |v: Option<f64>| v.map_or(s.null(), |v| s.number(v));
        result.set_atom("altitude", optional(fix.altitude))?;
        result.set_atom("altitudeAccuracy", optional(fix.altitude_accuracy))?;
        result.set_atom("heading", optional(fix.heading))?;
        result.set_atom("speed", optional(fix.speed))?;
        Ok(result.value())
    })?;
    native.set_atom("geolocationPosition", position.value())?;
    // Rooted while the factory runs (the functions are only referenced by
    // `native`, itself an argument).
    let root = Persistent::new(s, native.value());
    let factory =
        s.eval(include_str!("navigator_extras.js"), Some("pyro://internal/navigator-extras.js"), 1)?;
    let factory = factory.as_object().ok_or_else(|| JsError::Type("navigator extras factory".into()))?;
    let extras = factory.call(None, &[root.get(s)])?;
    extras.as_object().ok_or_else(|| JsError::Type("navigator extras".into()))
}

fn permission_state(page: &PageDom, name: &str) -> PermissionSetting {
    let origin = crate::storage::document_origin(page);
    page.context_services().state(origin.as_deref(), name)
}

/// The client hints `navigator.userAgentData` reports: the override's
/// metadata; none for a user agent overridden without metadata (as Chrome);
/// else the browser's own, derived from its user agent string. Fields the
/// metadata leaves out are filled by [`UserAgentMetadata::json`].
pub fn user_agent_metadata(page: &PageDom) -> UserAgentMetadata {
    let identity = page.identity.borrow();
    match identity.value.as_ref() {
        Some(v) if v.metadata.is_some() => v.metadata.clone().unwrap_or_default(),
        Some(v) if !v.user_agent.is_empty() => UserAgentMetadata::empty(),
        _ => {
            drop(identity);
            UserAgentMetadata::from_user_agent(&crate::window::user_agent(page), &crate::window::platform(page))
        }
    }
}

/// [`user_agent_metadata`] as JSON, with left-out fields taken from the
/// browser's own hints (its default agent, not the override's).
fn user_agent_json(page: &PageDom) -> String {
    user_agent_metadata(page).json(|| {
        let agent = page
            .net
            .borrow()
            .as_ref()
            .map(|n| n.user_agent())
            .unwrap_or_else(|| pyro_net::NetConfig::default().user_agent);
        UserAgentMetadata::from_user_agent(&agent, &crate::window::platform(page))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter().map(|(b, v)| (b.to_string(), v.to_string())).collect()
    }

    #[test]
    fn chrome_agents_get_chromium_brand_lists() {
        // Measured on headless Chrome 156 (HeadlessChrome agent, "Google
        // Chrome" brand): "Not:A-Brand";v="8", "Chromium";v="156", ...
        let ua = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) HeadlessChrome/156.0.8078.80 Safari/537.36";
        let m = UserAgentMetadata::from_user_agent(ua, "MacIntel");
        assert_eq!(m.brands, Some(pairs(&[("Not:A-Brand", "8"), ("Chromium", "156"), ("Google Chrome", "156")])));
        assert_eq!(
            m.full_version_list.unwrap(),
            pairs(&[("Not:A-Brand", "8.0.0.0"), ("Chromium", "156.0.8078.80"), ("Google Chrome", "156.0.8078.80")])
        );
        assert_eq!((m.platform.as_str(), m.mobile, m.full_version.as_deref()), ("macOS", false, Some("156.0.8078.80")));
        // Other majors change the GREASE brand and the order (seed % 6).
        let m = UserAgentMetadata::from_user_agent(
            "Mozilla/5.0 (Linux; Android 14) Chrome/121.0.0.0 Mobile Safari/537.36",
            "Linux armv8l",
        );
        assert_eq!(m.brands, Some(pairs(&[("Not A(Brand", "99"), ("Google Chrome", "121"), ("Chromium", "121")])));
        assert_eq!((m.platform.as_str(), m.mobile), ("Android", true));
        assert_eq!(
            UserAgentMetadata::from_user_agent("Mozilla/5.0 (compatible; pyro/1)", "MacIntel").brands,
            Some(Vec::new())
        );
    }

    #[test]
    fn client_hint_headers_are_structured_strings() {
        let metadata = UserAgentMetadata {
            brands: Some(pairs(&[("X", "1"), ("Q\"u\\o", "2")])),
            platform: "Android".into(),
            mobile: true,
            ..Default::default()
        };
        let headers: Vec<(String, String)> = crate::identity::client_hint_headers(&metadata)
            .into_iter()
            .map(|(n, v)| (n.to_string(), v.to_str().unwrap().to_owned()))
            .collect();
        assert_eq!(
            headers,
            pairs(&[
                ("sec-ch-ua", r#""X";v="1", "Q\"u\\o";v="2""#),
                ("sec-ch-ua-mobile", "?1"),
                ("sec-ch-ua-platform", "\"Android\""),
            ])
        );
        // No brands: no Sec-CH-UA.
        let none = crate::identity::client_hint_headers(&UserAgentMetadata::default());
        assert_eq!(
            none.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(),
            ["sec-ch-ua-mobile", "sec-ch-ua-platform"]
        );
    }

    #[test]
    fn permission_table_prefers_the_origin_then_every_origin_then_defaults() {
        let services = ContextServices::default();
        assert_eq!(services.state(None, "clipboard-write"), PermissionSetting::Granted);
        assert_eq!(services.state(Some("https://a.example"), "clipboard-read"), PermissionSetting::Prompt);
        services.grant(Some("https://a.example".into()), &["geolocation".into()]);
        assert_eq!(services.state(Some("https://a.example"), "geolocation"), PermissionSetting::Granted);
        assert_eq!(services.state(Some("https://a.example"), "clipboard-write"), PermissionSetting::Denied);
        assert_eq!(services.state(Some("https://b.example"), "geolocation"), PermissionSetting::Prompt);
        services.set(None, "geolocation", PermissionSetting::Denied);
        assert_eq!(services.state(Some("https://b.example"), "geolocation"), PermissionSetting::Denied);
        services.reset();
        assert_eq!(services.state(Some("https://a.example"), "geolocation"), PermissionSetting::Prompt);
    }
}
