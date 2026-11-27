//! Storage Access API (https://privacycg.github.io/storage-access/): `document.hasStorageAccess()`,
//! `document.requestStorageAccess()` (and the Chrome-shipped `hasUnpartitionedCookieAccess()` alias and the
//! `requestStorageAccess(types)` form), and the cookie decision they stand for.
//!
//! A document is a *third party* when it sits below a cross-site ancestor or is not at the top-level
//! document's site. Pyro does what Chrome does by default: third-party cookies are available. A browser
//! context blocks them for the origins its storage access rules name (`Browser.setStorageAccess`, which
//! testdriver's `set_storage_access` drives); with a rule that blocks, a third-party document has no
//! access to its unpartitioned cookies (requests send and store none, `document.cookie` is empty and
//! ignores writes: [`PageDom::cookie_blocking`], consulted by the net client and `document.cookie`)
//! until `requestStorageAccess()` is granted to it. The grant belongs to the document (its frame keeps
//! it over a same-origin navigation the frame starts: `pyro-browser`'s loader).
//!
//! `hasStorageAccess()` is exactly "the document's cookies are not blocked", so with no blocking rule it
//! is true everywhere (the WPT pages skip their cross-site checks then). A request of a third party
//! resolves when the document was granted before, or is a frame at the top-level site below a
//! cross-site frame (Chrome's automatic grant), or the `storage-access` permission is granted (it is
//! asked even where nothing blocks the document's cookies); a denied permission, a
//! sandbox without `allow-storage-access-by-user-activation`, a policy that disables `storage-access`,
//! an opaque origin and an insecure context reject with `NotAllowedError`. There is no prompt UI to
//! answer, so the `prompt` state rejects too (with or without a user gesture).
//!
//! The `StorageAccessTypes` form grants cookie access only: Pyro's Web Storage, IndexedDB, Cache Storage,
//! Web Locks and the like are not partitioned by top-level site for a third party, so there is no
//! partitioned/unpartitioned pair to hand a handle for. `{cookies: true}` alone resolves with a
//! `StorageAccessHandle` (a cookies-only request legitimately yields a handle without storage members: the
//! handle carries only what was requested besides cookies); `{all: true}` and asking for one of the storage
//! members by name reject with `NotSupportedError`, since the handle could not carry them. `requestStorageAccessFor()` (Related Website Sets) is not provided.

use std::rc::Rc;
use std::sync::Arc;

use pyro_dom::NodeId;
use pyro_js::{ClassDef, JsClass, JsError, JsResult, NoParent, Scope, Value};
use pyro_net::cookies::{partition_key, CookieBlocking, SiteContext};

use crate::navigator_extras::PermissionSetting;
use crate::page::PageDom;

/// The `StorageAccessHandle` a cookies-only typed request resolves with (no storage members were requested).
pub(crate) struct StorageAccessHandle;
impl JsClass for StorageAccessHandle {
    const NAME: &'static str = "StorageAccessHandle";
    type Data = ();
    type Parent = NoParent;
    fn define(_def: &mut ClassDef<Self>) {}
}

pub(crate) fn register(engine: &pyro_js::Engine) {
    engine.register::<StorageAccessHandle>();
}

/// The schemeful site of a document's origin (`None`: opaque or not http(s)).
fn own_site(page: &PageDom) -> Option<String> {
    let origin = crate::storage::document_origin(page)?;
    partition_key(&url::Url::parse(&origin).ok()?)
}

impl PageDom {
    /// The document's third-party cookie decision, or `None` when no rule of its browser context
    /// blocks anything (every context by default): then nothing is asked of a request, and a
    /// first-party or same-site request has no cost.
    pub fn cookie_blocking(&self) -> Option<Arc<CookieBlocking>> {
        let rules = self.context_services().cookie_access_rules();
        if rules.is_empty() {
            return None;
        }
        // Fail closed: a top-level site that cannot be named (an opaque or non-web top) is no site any
        // request is at, so every request is a third party and the rules decide.
        let top_site = self.cookie_partition_key().unwrap_or_default();
        Some(Arc::new(CookieBlocking::new(
            rules,
            top_site,
            own_site(self),
            self.cookie_site_context() == SiteContext::CrossSite,
            self.storage_access_granted.get(),
        )))
    }

    /// Whether the document's unpartitioned cookies are unavailable to it (`document.cookie`).
    pub(crate) fn cookies_blocked(&self) -> bool {
        let Some(blocking) = self.cookie_blocking() else { return false };
        url::Url::parse(&self.document_url()).is_ok_and(|url| blocking.blocks(&url))
    }

    /// Keeps the previous document's storage access (a same-origin navigation of the frame by
    /// itself) or drops it.
    pub fn set_storage_access_granted(&self, granted: bool) {
        self.storage_access_granted.set(granted);
    }

    /// Whether `requestStorageAccess()` was granted to the document.
    pub fn storage_access_granted(&self) -> bool {
        self.storage_access_granted.get()
    }
}

/// The `storage-access` permission setting for the document: made for its own origin or for the
/// top-level one it is embedded under (`ContextServices::storage_access_state`).
pub(crate) fn permission(page: &PageDom) -> PermissionSetting {
    let own = crate::storage::document_origin(page);
    let top = page.hooks().and_then(|h| h.top_level_origin()).or_else(|| own.clone());
    page.context_services().storage_access_state(own.as_deref(), top.as_deref())
}

/// A third party: below a cross-site ancestor, or not at the top-level document's site.
fn third_party(page: &PageDom) -> bool {
    if page.cookie_site_context() == SiteContext::CrossSite {
        return true;
    }
    match (own_site(page), page.cookie_partition_key()) {
        (Some(own), Some(top)) => own != top,
        _ => true,
    }
}

/// "has storage access": the document is allowed to use its unpartitioned cookies.
fn has_access(s: &Scope<'_>, page: &PageDom) -> bool {
    // A credentialless document is in an ephemeral partition: it has no unpartitioned storage to access.
    if page.is_credentialless() || page.has_opaque_origin() || !crate::secure_context::secure(s, page) || !crate::permissions_policy::feature_allowed(page, "storage-access") {
        return false;
    }
    !page.cookies_blocked()
}

/// A document of no browsing context (`createDocument`, `DOMParser`) or one that is no longer fully
/// active rejects.
fn require_active(page: &PageDom, document: NodeId) -> JsResult<()> {
    if document != page.document() || !page.is_fully_active() {
        return Err(JsError::dom("InvalidStateError", "The document is not fully active."));
    }
    Ok(())
}

fn resolved<'s>(s: &Scope<'s>, value: Value<'s>) -> JsResult<Value<'s>> {
    let d = s.deferred()?;
    let promise = d.promise.get(s);
    d.resolve(s, value)?;
    Ok(promise)
}

/// `document.hasStorageAccess()` (and `hasUnpartitionedCookieAccess()`).
pub(crate) fn has<'s>(s: &Scope<'s>, page: &Rc<PageDom>, document: NodeId) -> JsResult<Value<'s>> {
    require_active(page, document)?;
    resolved(s, s.boolean(has_access(s, page)))
}

/// The `StorageAccessTypes` members a handle would carry storage for.
const STORAGE_MEMBERS: [&str; 11] = [
    "sessionStorage", "localStorage", "indexedDB", "locks", "caches", "getDirectory", "estimate", "createObjectURL", "revokeObjectURL",
    "BroadcastChannel", "SharedWorker",
];

/// The `StorageAccessTypes` dictionary: `(all, cookies, the storage members named)`.
fn read_types(types: Value<'_>) -> JsResult<(bool, bool, Vec<&'static str>)> {
    let Some(o) = types.as_object() else {
        if types.is_nullish() {
            return Ok((false, false, Vec::new()));
        }
        return Err(JsError::Type("Failed to execute 'requestStorageAccess' on 'Document': parameter 1 is not of type 'StorageAccessTypes'.".into()));
    };
    let all = o.get_atom("all")?.to_bool();
    let cookies = o.get_atom("cookies")?.to_bool();
    let mut named = Vec::new();
    // Web IDL reads a dictionary's members in lexicographic order.
    let mut members = STORAGE_MEMBERS;
    members.sort_unstable();
    for member in members {
        if o.get_atom(member)?.to_bool() {
            named.push(member);
        }
    }
    Ok((all, cookies, named))
}

fn not_allowed<T>(message: &str) -> JsResult<T> {
    Err(JsError::dom("NotAllowedError", message))
}

/// `document.requestStorageAccess(types?)`. `types` is the first argument (undefined: the form
/// without one, which resolves with undefined).
pub(crate) fn request<'s>(s: &Scope<'s>, page: &Rc<PageDom>, document: NodeId, types: Value<'s>) -> JsResult<Value<'s>> {
    require_active(page, document)?;
    let typed = !types.is_undefined();
    let (all, cookies, named) = if typed { read_types(types)? } else { (false, false, Vec::new()) };
    if typed {
        if !all && !cookies && named.is_empty() {
            return not_allowed("requestStorageAccess() needs at least one storage type to be requested.");
        }
        // A handle carries the storage members that were requested (`all` is every one of them). Pyro's storage
        // is not partitioned for a third party, so there is nothing to put in such a handle: reject rather than
        // resolve with a handle whose members are missing.
        if all || !named.is_empty() {
            let what = if all { "all storage".to_string() } else { named.join(", ") };
            return Err(JsError::dom("NotSupportedError", format!("Storage access handles for {what} are not supported: that storage is not partitioned.")));
        }
    }
    grant(s, page)?;
    if typed {
        return resolved(s, s.wrap::<StorageAccessHandle>((), None)?.value());
    }
    resolved(s, s.undefined())
}

/// The checks of "request storage access" and the grant they lead to.
fn grant(s: &Scope<'_>, page: &Rc<PageDom>) -> JsResult<()> {
    if page.is_credentialless() {
        return not_allowed("requestStorageAccess() is not allowed in a credentialless iframe.");
    }
    if page.has_opaque_origin() {
        return not_allowed("requestStorageAccess() is not allowed in a document with an opaque origin.");
    }
    if !crate::secure_context::secure(s, page) {
        return not_allowed("requestStorageAccess() is only allowed in secure contexts.");
    }
    // A document that is not a third party has its cookies already, and one that was granted keeps them.
    // (A third party is asked even where nothing blocks its cookies, as Chrome does.)
    if !third_party(page) || page.storage_access_granted() {
        return Ok(());
    }
    if page.storage_access_sandboxed() {
        return not_allowed("requestStorageAccess() is not allowed in a sandboxed iframe without allow-storage-access-by-user-activation.");
    }
    if !crate::permissions_policy::check_feature(s, page, "storage-access") {
        return not_allowed("requestStorageAccess() is disabled by the storage-access permissions policy.");
    }
    // Chrome grants a frame at the top-level site (below a cross-site one) without asking.
    let same_site_as_top = own_site(page).is_some() && own_site(page) == page.cookie_partition_key();
    if !same_site_as_top {
        match permission(page) {
            PermissionSetting::Granted => {}
            PermissionSetting::Denied => return not_allowed("The storage-access permission was denied."),
            // No prompt can be shown here: asking would be dismissed, as the denial it is.
            PermissionSetting::Prompt if !page.has_transient_activation() => {
                return not_allowed("requestStorageAccess() needs a user gesture or a granted storage-access permission.");
            }
            PermissionSetting::Prompt => return not_allowed("The storage-access permission prompt cannot be answered."),
        }
    }
    page.set_storage_access_granted(true);
    Ok(())
}
