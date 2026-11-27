//! Encrypted Media Extensions with the Clear Key key system (`org.w3.clearkey`). The interfaces
//! (`MediaKeySystemAccess`, `MediaKeys`, `MediaKeySession`, `MediaKeyStatusMap`, the two events and
//! `HTMLMediaElement.setMediaKeys`) are made in JS (eme.js). What is native is what they hand to
//! playback: a [`KeyStore`] per `MediaKeys` holding the keys its sessions were given by `update()`, which
//! a media element with that `MediaKeys` decrypts its Common Encryption samples with
//! (`media::mse_playback`, `pyro_media::cenc`) before they reach the decoder, and the `encrypted` event
//! the element fires for the protection information of an MSE init segment.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use aes::cipher::{BlockDecrypt, BlockEncrypt, KeyInit};
use pyro_dom::NodeId;
use pyro_js::{ClassDef, JsClass, JsError, JsResult, Object, Scope, Value};
use pyro_media::cenc::BlockCipher;
use pyro_media::mse_buffer::MediaType;

use crate::lazy_module::{lazy_globals, LazyModule};
use crate::page::PageDom;
use crate::window::WindowHost;

pub(crate) static MODULE: LazyModule = LazyModule {
    key: "lpEme",
    source: include_str!("eme.js"),
    url: "pyro://internal/eme.js",
    natives: Some(natives),
};

/// The interface objects of eme.js on a window global (`crate::secure_context` removes the
/// `[SecureContext]` ones from an insecure window).
pub(crate) fn define_globals<W: JsClass>(def: &mut ClassDef<W>)
where
    W::Data: WindowHost,
{
    lazy_globals!(
        def,
        MODULE,
        "MediaKeySystemAccess",
        "MediaKeys",
        "MediaKeySession",
        "MediaKeyStatusMap",
        "MediaKeyMessageEvent",
        "MediaEncryptedEvent"
    );
}

/// The keys the sessions of one `MediaKeys` can decrypt with: per session, `(key ID, key)` of the
/// keys whose status is `usable`.
#[derive(Default)]
pub(crate) struct KeyStore {
    sessions: HashMap<u32, Vec<([u8; 16], [u8; 16])>>,
}

impl KeyStore {
    /// The usable key of this key ID in any session.
    pub(crate) fn key(&self, kid: &[u8; 16]) -> Option<[u8; 16]> {
        self.sessions.values().flatten().find(|(id, _)| id == kid).map(|(_, key)| *key)
    }
}

pub(crate) type SharedKeys = Rc<RefCell<KeyStore>>;

thread_local! {
    /// The stores of the live `MediaKeys` objects by the number eme.js knows them by (its
    /// finalization registry drops the entry, or [`release_document`] when the page goes); an element
    /// using one holds its own reference. Each entry knows the page that made it.
    static STORES: RefCell<(u32, HashMap<u32, (u64, SharedKeys)>)> = RefCell::new((0, HashMap::new()));
}

fn lookup(id: f64) -> Option<SharedKeys> {
    STORES.with(|stores| stores.borrow().1.get(&(id as u32)).map(|(_, keys)| keys.clone()))
}

/// The page is disposed: its key stores go with it (its realms' finalization registries may never run).
pub(crate) fn release_document(page: &PageDom) {
    let id = page.id();
    STORES.with(|stores| stores.borrow_mut().1.retain(|_, (owner, _)| *owner != id));
}

/// AES-128 as the Common Encryption block cipher.
pub(crate) struct Aes(aes::Aes128);

impl Aes {
    pub(crate) fn new(key: &[u8; 16]) -> Self {
        Self(aes::Aes128::new_from_slice(key).expect("16-byte key"))
    }
}

impl BlockCipher for Aes {
    fn encrypt_block(&self, block: &mut [u8; 16]) {
        self.0.encrypt_block(aes::Block::from_mut_slice(block));
    }
    fn decrypt_block(&self, block: &mut [u8; 16]) {
        self.0.decrypt_block(aes::Block::from_mut_slice(block));
    }
}

/// What Clear Key can play of `contentType`: bit 0 audio, bit 1 video, 0 when nothing. Beyond what
/// MSE accepts (`MediaType`: fragmented MP4, H.264 and AAC), EME needs the `codecs` parameter and no
/// other (`video/mp4; foo="bar"` names no supported type).
fn content_type_kinds(content_type: &str) -> u8 {
    let mut parts = content_type.split(';');
    parts.next();
    let mut codecs = false;
    for param in parts {
        let param = param.trim();
        if param.is_empty() {
            continue;
        }
        let Some((name, value)) = param.split_once('=') else { return 0 };
        if !name.trim().eq_ignore_ascii_case("codecs") || value.trim().is_empty() {
            return 0;
        }
        codecs = true;
    }
    if !codecs {
        return 0;
    }
    MediaType::parse(content_type).map_or(0, |ty| u8::from(ty.has_audio()) | (u8::from(ty.has_video()) << 1))
}

fn natives<'s>(s: &Scope<'s>) -> JsResult<Object<'s>> {
    let native = s.object();
    crate::video_decoder::install_queue_tasks(s, &native)?;
    native.set_atom(
        "kinds",
        s.function("kinds", |s, _, a| Ok(s.number(f64::from(content_type_kinds(&a.get(0).to_string()?)))))?.value(),
    )?;
    // The origin of the document is opaque (a sandboxed frame): no MediaKeys there.
    native.set_atom(
        "opaque",
        s.function("opaque", |s, _, _| {
            let page = crate::target::page_of(s)?;
            Ok(s.boolean(crate::storage::tuple_origin(s, &page).is_none()))
        })?
        .value(),
    )?;
    native.set_atom(
        "makeStore",
        s.function("makeStore", |s, _, _| {
            let owner = crate::target::page_of(s)?.id();
            let id = STORES.with(|stores| {
                let mut stores = stores.borrow_mut();
                stores.0 += 1;
                let id = stores.0;
                stores.1.insert(id, (owner, Rc::new(RefCell::new(KeyStore::default()))));
                id
            });
            Ok(s.number(f64::from(id)))
        })?
        .value(),
    )?;
    native.set_atom(
        "dropStore",
        s.function("dropStore", |s, _, a| {
            let id = a.get(0).to_number()? as u32;
            STORES.with(|stores| stores.borrow_mut().1.remove(&id));
            Ok(s.undefined())
        })?
        .value(),
    )?;
    // setKeys(store, session, bytes): the usable keys of a session as 32-byte `kid || key` records,
    // which the elements playing with this MediaKeys then decrypt with (or stop on).
    native.set_atom(
        "setKeys",
        s.function("setKeys", |s, _, a| {
            let session = a.get(1).to_number()? as u32;
            let bytes = crate::encoding::buffer_source_bytes(s, a.get(2))?.ok_or(JsError::ContextDestroyed)?;
            let Some(store) = lookup(a.get(0).to_number()?) else { return Ok(s.undefined()) };
            let pairs: Vec<([u8; 16], [u8; 16])> = bytes
                .chunks_exact(32)
                .map(|record| {
                    let (mut kid, mut key) = ([0u8; 16], [0u8; 16]);
                    kid.copy_from_slice(&record[..16]);
                    key.copy_from_slice(&record[16..]);
                    (kid, key)
                })
                .collect();
            {
                let mut store = store.borrow_mut();
                if pairs.is_empty() {
                    store.sessions.remove(&session);
                } else {
                    store.sessions.insert(session, pairs);
                }
            }
            let page = crate::target::page_of(s)?;
            crate::media::keys_changed(&page, &store);
            Ok(s.undefined())
        })?
        .value(),
    )?;
    // attach(element, store | null): the element's MediaKeys (`setMediaKeys`).
    native.set_atom(
        "attach",
        s.function("attach", |s, _, a| {
            let page = crate::target::page_of(s)?;
            let node = crate::node::node_arg(s, a.get(0), "setMediaKeys")?;
            let store = if a.get(1).is_null() { None } else { lookup(a.get(1).to_number()?) };
            crate::media::set_media_keys(&page, node, store);
            Ok(s.undefined())
        })?
        .value(),
    )?;
    Ok(native)
}

/// `navigator.requestMediaKeySystemAccess(keySystem, supportedConfigurations)`.
pub(crate) fn request_access<'s>(
    s: &Scope<'s>,
    page: &Rc<PageDom>,
    argc: usize,
    key_system: Value<'s>,
    configurations: Value<'s>,
) -> JsResult<Value<'s>> {
    if page.is_disposed() {
        return Err(JsError::ContextDestroyed);
    }
    MODULE.helper(s, page, "requestAccess", &[s.number(argc as f64), key_system, configurations])
}

/// `HTMLMediaElement.setMediaKeys(mediaKeys)`.
pub(crate) fn set_media_keys<'s>(
    s: &Scope<'s>,
    page: &Rc<PageDom>,
    node: NodeId,
    media_keys: Value<'s>,
    argc: usize,
) -> JsResult<Value<'s>> {
    let element = crate::node::node_value(s, page, Some(node))?;
    MODULE.helper(s, page, "setMediaKeys", &[element, media_keys, s.number(argc as f64)])
}

/// `HTMLMediaElement.mediaKeys`: null until `setMediaKeys` made the module.
pub(crate) fn media_keys<'s>(s: &Scope<'s>, page: &Rc<PageDom>, node: NodeId) -> JsResult<Value<'s>> {
    if MODULE.existing(s, page)?.is_none() {
        return Ok(s.null());
    }
    let element = crate::node::node_value(s, page, Some(node))?;
    MODULE.helper(s, page, "getMediaKeys", &[element])
}

/// Fires `encrypted` (a MediaEncryptedEvent) at the element: its protected media's initialization data.
pub(crate) fn fire_encrypted(page: &Rc<PageDom>, node: NodeId, init_data_type: &str, init_data: &[u8]) {
    let Some(realm) = page.main_world_realm() else { return };
    let Ok(s) = realm.scope() else { return };
    let hex: String = init_data.iter().map(|b| format!("{b:02x}")).collect();
    let fired = crate::node::node_value(&s, page, Some(node)).and_then(|element| {
        MODULE.helper(&s, page, "fireEncrypted", &[element, s.string(init_data_type), s.string(&hex)])
    });
    if let Err(error) = fired {
        s.report_exception(&error);
    }
}
