//! Credentialless iframes (HTML "anonymous iframe", the `credentialless` attribute of `<iframe>`).
//!
//! A frame whose container has `credentialless`, and every frame below it, belong to the *anonymous
//! partition* of their top-level document: a nonce made when the first credentialless frame of that
//! document needs it, which lasts as long as the top-level document and is shared by all its
//! credentialless frames. Everything a document of the partition reads or writes in the following
//! storage areas lives under the nonce instead of the browser context's:
//!
//! - cookies: requests (`RequestDefaults::ephemeral_cookies`), `document.cookie`, the Cookie Store
//!   API and workers of the document use an empty jar of their own ([`Anonymous::cookies`]), so a
//!   credentialless request carries no cookie of the normal partition and the cookies it stores
//!   never reach it;
//! - Web Storage (`localStorage`, `sessionStorage`), IndexedDB, Cache Storage, OPFS and storage
//!   buckets: the document gets a fresh [`StorageNamespace`] (and so do its dedicated workers, which
//!   take their owner's);
//! - Web Locks, `BroadcastChannel` and shared workers: the storage key gets the nonce
//!   ([`PageDom::storage_partition_key`]).
//!
//! Not partitioned: service worker registrations and clients (the registry matches navigations by the
//! site only), the HTTP cache (a credentialless request may be answered from a response cached for a
//! credentialed one).
//!
//! A credentialless document has no storage access (`document.hasStorageAccess()` is false and
//! `requestStorageAccess()` rejects), a popup it opens has no opener (as `noopener`), and a popup
//! does not inherit the property. A frame of a document whose COEP is not `unsafe-none` may embed a
//! credentialless frame whose document has no COEP.

use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

use pyro_net::cookies::EphemeralCookies;

use crate::page::PageDom;
use crate::storage::StorageNamespace;

static NEXT_NONCE: AtomicU64 = AtomicU64::new(1);

/// What a document or worker of the anonymous partition needs from it, and can take across
/// threads (a worker's scope runs on its own thread): the nonce and the partition's cookie jar.
#[derive(Clone, Debug)]
pub struct Anonymous {
    pub nonce: u64,
    pub cookies: EphemeralCookies,
}

/// The anonymous partition of one top-level document.
pub struct CredentiallessPartition {
    anonymous: Anonymous,
    local: StorageNamespace,
    session: StorageNamespace,
}

impl CredentiallessPartition {
    /// A new, empty partition with a nonce of its own.
    pub fn new() -> Rc<Self> {
        Rc::new(Self {
            anonymous: Anonymous { nonce: NEXT_NONCE.fetch_add(1, Ordering::Relaxed), cookies: EphemeralCookies::default() },
            local: StorageNamespace::default(),
            session: StorageNamespace::default(),
        })
    }

    pub fn anonymous(&self) -> Anonymous {
        self.anonymous.clone()
    }

    /// The partition's `localStorage`, IndexedDB, Cache Storage, ... namespace.
    pub fn local(&self) -> StorageNamespace {
        self.local.clone()
    }

    /// The partition's `sessionStorage` namespace (shared by all its frames).
    pub fn session(&self) -> StorageNamespace {
        self.session.clone()
    }
}

impl PageDom {
    /// Puts the document (or worker) in the anonymous partition `anonymous` names, or takes it out.
    /// The storage namespaces are the embedder's to set (`PageDom::set_storage_namespaces`).
    pub fn set_anonymous(&self, anonymous: Option<Anonymous>) {
        *self.anonymous.borrow_mut() = anonymous;
    }

    /// The anonymous partition the document belongs to: it is credentialless.
    pub fn anonymous(&self) -> Option<Anonymous> {
        self.anonymous.borrow().clone()
    }

    /// HTML "credentialless": the document is in a credentialless iframe tree (`window.credentialless`).
    pub fn is_credentialless(&self) -> bool {
        self.anonymous.borrow().is_some()
    }

    /// The anonymous partition of the frame tree this top-level document roots, made on first use.
    /// Call it on the top-level document's `PageDom`.
    pub fn credentialless_partition(&self) -> Rc<CredentiallessPartition> {
        self.credentialless_root.get_or_init(CredentiallessPartition::new).clone()
    }

    /// The top-level site of the document's storage key for the areas that are keyed by it apart from
    /// the namespaces (Web Locks, `BroadcastChannel`, shared workers): the cookie partition key, plus
    /// the partition's nonce for a credentialless document.
    pub fn storage_partition_key(&self) -> Option<String> {
        let key = self.cookie_partition_key();
        match self.anonymous.borrow().as_ref() {
            Some(anonymous) => Some(format!("{}#credentialless:{}", key.unwrap_or_default(), anonymous.nonce)),
            None => key,
        }
    }
}

impl PageDom {
    /// The cookie jar the document's cookie APIs (`document.cookie`, the Cookie Store API) use: its
    /// credentialless partition's, else `net`'s.
    pub(crate) fn cookie_jar(&self, net: &pyro_net::NetClient) -> std::sync::Arc<std::sync::Mutex<pyro_net::cookies::CookieJar>> {
        match self.anonymous.borrow().as_ref() {
            Some(anonymous) => anonymous.cookies.0.clone(),
            None => net.cookies().clone(),
        }
    }
}
