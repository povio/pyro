//! `<img>` resources: fetched through the page's resource loader (Image
//! destination, so CDP sees an Image request) to learn the natural size
//! from the file header. Pixels are never decoded or painted.
//!
//! - A connected `<img src>` starts a load when its resolved URL changes. An
//!   unchanged `src` does not re-request.
//! - `blob:` URLs read the page's blob store; `data:` URLs go through the
//!   network client.
//! - While loading, `complete` is false. Images that start before the
//!   window `load` event delay it, as in Chromium.
//! - Completion records the natural size (`lp_dom::image`) and fires
//!   `load`. Network failure, a non-2xx status or a body that is neither a
//!   recognized image nor `image/*` fires `error`. An `image/*` body whose
//!   format has no header parser (for example AVIF) loads with an unknown
//!   size. SVG takes its size from numeric `width`/`height` on the root.
//! - An empty `src` fires `error`; removing `src` clears the image. Not
//!   modelled: the `<img>`'s own `srcset`/`sizes` selection,
//!   `loading="lazy"` deferral (lazy images load eagerly), `decode()` timing,
//!   images that are not connected (`new Image()`).
//! - An `<img>` child of a `<picture>` loads the first preceding `<source>`
//!   whose `media` matches and whose `type` is an image type
//!   (`lp_dom::picture`). Moving, inserting or removing a `<source>` or the
//!   `<img>` selects again and reloads when the URL changed.
use crate::frames::FrameKey;
use crate::loader::ResourceNode;
use crate::{Browser, FetchRequest, LoaderId, Resource, ResourceCancellation, ResourceKind};
use lp_dom::NodeId;
use lp_runtime::TaskSource;
use lp_webapi::PageDom;
use std::collections::HashMap;
use std::rc::Rc;

/// Bytes kept from an image response for header parsing.
const MAX_IMAGE_BYTES: usize = 64 * 1024 * 1024;

struct Image {
    _node: Rc<ResourceNode>,
    url: String,
    /// The `src` attribute value this load is for.
    src: String,
    generation: u64,
    pending: bool,
    blocks_load: bool,
    cancel: Option<ResourceCancellation>,
}

#[derive(Default)]
pub(crate) struct Images {
    entries: HashMap<NodeId, Image>,
    generation: u64,
}

impl Images {
    /// Pending images that started before the window `load` event.
    pub(crate) fn blocking_load(&self) -> usize {
        self.entries.values().filter(|i| i.pending && i.blocks_load).count()
    }
}

enum Source {
    None,
    Invalid,
    Url(String),
}

/// SVG natural size from numeric (px or unitless) root width/height.
fn svg_size(body: &[u8]) -> Option<(f64, f64)> {
    let text = std::str::from_utf8(body.get(..body.len().min(4096))?).ok()?;
    let start = text.find("<svg")?;
    let tag = &text[start..start + text[start..].find('>')?];
    let attr = |name: &str| {
        let i = tag.find(&format!(" {name}="))? + name.len() + 2;
        let quote = tag[i..].chars().next()?;
        let rest = &tag[i + 1..];
        let value = &rest[..rest.find(quote)?];
        value.trim().trim_end_matches("px").parse::<f64>().ok().filter(|v| *v > 0.0)
    };
    Some((attr("width")?, attr("height")?))
}

impl Browser {
    pub(crate) fn image_event(&mut self, page: FrameKey, node: NodeId) {
        let Some(state) = self.doc(page) else { return };
        let Some(bindings) = state.main_frame.page_dom.clone() else { return };
        let loader = state.main_frame.loader_id.clone();
        let base = state.main_frame.base_url.clone();
        let loaded = state.main_frame.loaded;
        // Inspect the final state: several src writes in one task start one load.
        let raw_src = bindings.dom.borrow().get_attribute(node, "src").ok().flatten().map(str::to_owned);
        // A `<picture>` parent's matching `<source>` provides the image
        // instead of `src` (a `<source>` or the `<img>` moving in or out of
        // the picture is a relevant mutation: the DOM queues this event).
        let picture_source = bindings.dom.borrow().picture_source(node, &bindings.viewport());
        let source = {
            let dom = bindings.dom.borrow();
            match picture_source.as_deref().or(dom.get_attribute(node, "src").ok().flatten()) {
                None => Source::None,
                Some(v) if v.trim().is_empty() => Source::Invalid,
                Some(v) => url::Url::parse(&base)
                    .ok()
                    .and_then(|b| b.join(v.trim()).ok())
                    .map_or(Source::Invalid, |u| Source::Url(u.to_string())),
            }
        };
        let current = state.images.entries.get(&node).map(|i| i.url.clone());
        drop(state);
        let url = match source {
            Source::Url(url) if current.as_deref() == Some(url.as_str()) => return,
            Source::Url(url) => url,
            Source::None | Source::Invalid => {
                let invalid = matches!(source, Source::Invalid);
                self.doc_mut(page).expect("page").images.entries.remove(&node);
                bindings.set_image_state(node, None, false);
                bindings.dom.borrow_mut().set_natural_size(node, None);
                if invalid {
                    self.image_error_task(page, loader, &bindings, node);
                }
                return;
            }
        };
        if bindings.dom.borrow_mut().add_ref(node).is_err() {
            return;
        }
        let hold = Rc::new(ResourceNode { dom: bindings.dom.clone(), node });
        let generation = {
            let mut guard = self.doc_mut(page).expect("page");
            let images = &mut guard.images;
            images.generation += 1;
            let generation = images.generation;
            // Replacing an entry drops its cancellation, stopping the old request.
            images.entries.insert(
                node,
                Image {
                    _node: hold,
                    url: url.clone(),
                    src: raw_src.clone().unwrap_or_default(),
                    generation,
                    pending: true,
                    blocks_load: !loaded,
                    cancel: None,
                },
            );
            generation
        };
        bindings.set_image_state(node, raw_src.as_deref(), true);
        bindings.dom.borrow_mut().set_natural_size(node, None);
        let weak = Rc::downgrade(&bindings);
        let finish = move |b: &mut Browser, result| {
            let Some(bindings) = weak.upgrade() else { return };
            b.finish_image(page, &loader, &bindings, node, generation, result);
        };
        if url.starts_with("blob:") {
            let result = bindings
                .blob_url(&url)
                .map(|(bytes, mime)| Resource {
                    url: url.clone(),
                    status: 200,
                    mime: mime.split(';').next().unwrap_or("").trim().to_ascii_lowercase(),
                    charset: None,
                    body: bytes.to_vec(),
                    content_disposition: None,
                    header_csp: false,
                })
                .ok_or_else(|| "revoked or unknown blob URL".to_owned());
            self.scheduler().post(TaskSource::Networking, move |b| finish(b, result));
            return;
        }
        let Some(resource_loader) = self.loader_for(page) else {
            self.scheduler().post(TaskSource::Networking, move |b| finish(b, Err("no resource loader".into())));
            return;
        };
        let frame = self.doc(page).expect("page").main_frame.clone();
        let request = FetchRequest {
            post: None,
            url,
            kind: ResourceKind::Image,
            page: page.page,
            referrer: Some(base),
            frame_id: Some(frame.id.0.clone()),
            loader_id: Some(frame.loader_id.0.clone()),
            crossorigin: None,
        };
        let cancel = resource_loader.fetch_cancellable(self, request, Box::new(finish));
        if let Some(mut image) = self.doc_mut(page).and_then(|p| p.filter_map(|p| p.images.entries.get_mut(&node))) {
            image.cancel = cancel;
        }
    }

    fn finish_image(
        &mut self,
        page: FrameKey,
        loader: &LoaderId,
        bindings: &Rc<PageDom>,
        node: NodeId,
        generation: u64,
        result: Result<Resource, String>,
    ) {
        // A src change queued in this task supersedes this result.
        self.drain_dom_resources(page);
        let Some(mut state) = self.doc_mut(page).filter(|p| &p.main_frame.loader_id == loader) else { return };
        let Some(image) = state.images.entries.get_mut(&node).filter(|i| i.generation == generation && i.pending)
        else {
            return;
        };
        if let Some(cancel) = image.cancel.take() {
            cancel.disarm();
        }
        image.pending = false;
        let src = image.src.clone();
        drop(state);
        let outcome = result.ok().filter(|r| (200..300).contains(&r.status) && !r.body.is_empty()).and_then(|r| {
            let body = &r.body[..r.body.len().min(MAX_IMAGE_BYTES)];
            if let Some((w, h)) = lp_dom::image::dimensions(body) {
                return Some(Some((w as f64, h as f64)));
            }
            if r.mime == "image/svg+xml" || body.trim_ascii_start().starts_with(b"<svg") {
                return Some(svg_size(body));
            }
            r.mime.starts_with("image/").then_some(None)
        });
        bindings.dom.borrow_mut().set_natural_size(node, outcome.flatten());
        bindings.set_image_state(node, Some(&src), false);
        // A later owner-thread task: no DOM/state borrow survives dispatch.
        lp_webapi::lifecycle::resource_loaded(bindings, node, outcome.is_some());
        self.maybe_fire_load(page);
    }

    fn image_error_task(&self, page: FrameKey, loader: LoaderId, bindings: &Rc<PageDom>, node: NodeId) {
        if bindings.dom.borrow_mut().add_ref(node).is_err() {
            return;
        }
        let hold = ResourceNode { dom: bindings.dom.clone(), node };
        let weak = Rc::downgrade(bindings);
        self.scheduler().post(TaskSource::Networking, move |b| {
            let _pin = &hold;
            if b.doc(page).is_some_and(|p| p.main_frame.loader_id == loader) {
                if let Some(bindings) = weak.upgrade() {
                    lp_webapi::lifecycle::resource_loaded(&bindings, hold.node, false);
                }
            }
        });
    }
}
