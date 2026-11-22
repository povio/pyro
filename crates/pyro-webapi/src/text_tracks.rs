//! Per-realm text track interfaces and private hooks for the media loader.
use std::rc::Rc;

use pyro_js::{ClassDef, JsClass, JsError, JsResult, Object, Persistent, Scope, Value};

use crate::lazy_module::{lazy_globals, LazyModule};
use crate::node::{node_value, Node, NodeData};
use crate::page::PageDom;
use crate::window::WindowHost;

static MODULE: LazyModule = LazyModule {
    key: "lpTextTracks",
    source: concat!(
        include_str!("text_tracks.js"),
        "(",
        include_str!("webvtt_cue.js"),
        ",",
        include_str!("webvtt_file.js"),
        ")"
    ),
    url: "pyro://internal/text_tracks.js",
    natives: Some(natives),
};

pub(crate) fn define_globals<W: JsClass>(def: &mut ClassDef<W>)
where
    W::Data: WindowHost,
{
    lazy_globals!(
        def,
        MODULE,
        "TextTrack",
        "TextTrackList",
        "TextTrackCueList",
        "TrackEvent",
        "TextTrackCue",
        "VTTCue",
        "VTTRegion"
    );
}

pub(crate) fn helper<'s>(s: &Scope<'s>, page: &Rc<PageDom>, name: &str, args: &[Value<'s>]) -> JsResult<Value<'s>> {
    MODULE.helper(s, page, name, args)
}

pub(crate) fn element_helper<'s>(s: &Scope<'s>, n: &NodeData, name: &str, extra: &[Value<'s>]) -> JsResult<Value<'s>> {
    let mut args = vec![node_value(s, n.page(), Some(n.id))?];
    args.extend_from_slice(extra);
    helper(s, n.page(), name, &args)
}

/// Look up private slots in the object's creating realm; never use public
/// properties or prototypes for cross-realm Web IDL brand checks.
fn foreign_state<'s>(s: &Scope<'s>, kind: &str, value: Value<'s>) -> JsResult<Value<'s>> {
    if !matches!(kind, "cue" | "region" | "track") {
        return Ok(s.undefined());
    }
    let Some(realm) = value.as_object().and_then(|object| s.object_realm(object)) else { return Ok(s.undefined()) };
    if realm.id() == s.realm_id() || realm.is_disposed() {
        return Ok(s.undefined());
    }
    let home = realm.scope()?;
    let page = crate::target::page_of(&home)?;
    if page.origin_key() != crate::target::page_of(s)?.origin_key() {
        return Ok(s.undefined());
    }
    let Some(module) = MODULE.existing(&home, &page)? else { return Ok(s.undefined()) };
    let lookup = module.get_atom("localState")?.as_object().ok_or(JsError::ContextDestroyed)?;
    let result = lookup.call(None, &[home.string(kind), home.rehome(value)])?;
    Ok(Persistent::new(&home, result).get(s))
}

/// Only enter JS for an existing media wrapper and track module.
fn on_media<T>(
    page: &Rc<PageDom>,
    node: pyro_dom::NodeId,
    run: impl for<'s> FnOnce(&Scope<'s>, Object<'s>, Value<'s>) -> JsResult<T>,
) -> Option<T> {
    let realm = page.main_world_realm()?;
    let s = realm.scope().ok()?;
    let wrapper = page.node_wrapper(&s, node)?;
    let module = MODULE.existing(&s, page).ok()??;
    match run(&s, module, wrapper.value()) {
        Ok(value) => Some(value),
        Err(error) => {
            s.report_exception(&error);
            None
        }
    }
}

pub(crate) fn playback(page: &Rc<PageDom>, node: pyro_dom::NodeId, normal: bool) -> bool {
    on_media(page, node, |s, module, media| {
        let callback = module.get_atom("advancePlayback")?.as_object().ok_or(JsError::ContextDestroyed)?;
        Ok(callback.call(None, &[media, s.boolean(normal)])?.to_bool())
    })
    .unwrap_or(false)
}

/// A pause-on-exit cue is a media deadline even between periodic timeupdates.
pub(crate) fn next_pause(page: &Rc<PageDom>, node: pyro_dom::NodeId, position: f64) -> f64 {
    on_media(page, node, |s, module, media| {
        let callback = module.get_atom("nextPause")?.as_object().ok_or(JsError::ContextDestroyed)?;
        callback.call(None, &[media, s.number(position)])?.to_number()
    })
    .unwrap_or(f64::INFINITY)
}

fn natives<'s>(s: &Scope<'s>) -> JsResult<Object<'s>> {
    let methods = s.object();
    methods.set_atom(
        "foreignState",
        s.function("foreignState", |s, _, a| foreign_state(s, &a.get(0).to_string()?, a.get(1)))?.value(),
    )?;
    methods.set_atom(
        "decodeCueText",
        s.function("decodeCueText", |s, _, a| {
            Ok(s.string(&pyro_dom::prescan::decode_webvtt_references(&a.get(0).to_string()?)))
        })?
        .value(),
    )?;
    methods.set_atom(
        "children",
        s.function("trackChildren", |s, _, a| {
            let n = s.unwrap::<Node>(a.get(0)).ok_or(JsError::ContextDestroyed)?;
            let children: Vec<_> = {
                let dom = n.page().dom.borrow();
                dom.element_children(n.id)
                    .into_iter()
                    .flatten()
                    .filter(|id| dom.element(*id).is_ok_and(|e| e.is_html() && &*e.name.local == "track"))
                    .collect()
            };
            let result = s.array(&[])?;
            for (i, id) in children.into_iter().enumerate() {
                result.set_index(i as u32, node_value(s, n.page(), Some(id))?)?;
            }
            Ok(result.value())
        })?
        .value(),
    )?;
    methods.set_atom(
        "sourceURL",
        s.function("trackSourceURL", |s, _, a| {
            let n = s.unwrap::<Node>(a.get(0)).ok_or(JsError::ContextDestroyed)?;
            Ok(s.string(&crate::media::track_url(n.page(), n.id)))
        })?.value(),
    )?;
    methods.set_atom(
        "check",
        s.function("checkTrackResource", |s, _, a| {
            let n = s.unwrap::<Node>(a.get(0)).ok_or(JsError::ContextDestroyed)?;
            crate::media::check_track_resource(n.page(), n.id);
            Ok(s.undefined())
        })?
        .value(),
    )?;
    methods.set_atom(
        "fire",
        s.function("fireTrackEvent", |s, _, a| {
            let t = s.unwrap::<crate::target::EventTarget>(a.get(0)).ok_or(JsError::ContextDestroyed)?;
            let ev = s.unwrap::<crate::event::Event>(a.get(1)).ok_or(JsError::ContextDestroyed)?;
            ev.state.borrow_mut().is_trusted = true;
            crate::target::fire(&t.page, t.key, &ev);
            Ok(s.undefined())
        })?
        .value(),
    )?;
    crate::video_decoder::install_queue_tasks(s, &methods)?;
    Ok(methods)
}
