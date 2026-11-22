//! Per-realm text track interfaces and private hooks for the media loader.
use std::rc::Rc;

use pyro_js::{ClassDef, JsClass, JsError, JsResult, Object, Scope, Value};

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

fn natives<'s>(s: &Scope<'s>) -> JsResult<Object<'s>> {
    let methods = s.object();
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
