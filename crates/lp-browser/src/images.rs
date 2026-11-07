//! `<img>` resources: fetched through the page's resource loader (Image
//! destination, so CDP sees an Image request) to learn the natural size
//! from the file header. Pixels are not decoded here: when the document may
//! draw images on a canvas, the bytes are kept for it to decode on use
//! (`PageDom::set_image_bytes`).
//!
//! - An `<img src>` starts a load when its resolved URL (against the
//!   document base URL, so `<base href>` counts) changes, connected or not:
//!   `new Image()` and an `<img>` built before insertion load as in Chromium,
//!   when their document has a browsing context. The element is pinned while
//!   its request is in flight and released when it completes. An unchanged
//!   `src` does not re-request.
//! - `decode()` (`lp_webapi::image_decode`) fulfills once the current
//!   request completed with a recognized image, and rejects with
//!   `EncodingError` for a broken one, a source changed before completion,
//!   no source, or an inactive document.
//! - `blob:` URLs read the page's blob store; `data:` URLs go through the
//!   network client.
//! - While loading, `complete` is false. Images that start before the
//!   window `load` event delay it, as in Chromium.
//! - Completion records the natural size (`lp_dom::image`) and fires
//!   `load`. Network failure, a non-2xx status or a body that is neither a
//!   recognized image nor `image/*` fires `error`. An `image/*` body whose
//!   format has no header parser (for example AVIF) loads with an unknown
//!   size. SVG takes its size from numeric `width`/`height` on the root.
//! - An empty `src` fires `error`; removing `src` clears the image. The
//!   `<img>`'s own `srcset` (with `sizes`) selects the candidate for the
//!   device pixel ratio (`lp_dom::picture`). Not modelled:
//!   `loading="lazy"` deferral (lazy images load eagerly), re-resolving a
//!   loaded `src` when a `<base>` is inserted later.
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
    /// Pins the element while its request is in flight (a `new Image()`
    /// nobody references still fires `load`); released when it completes.
    _node: Option<Rc<ResourceNode>>,
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
    /// Entry count at which those of collected elements are dropped (a
    /// finished request no longer pins its element, and `new Image()`
    /// beacons would otherwise accumulate): amortized O(1) per load.
    sweep_at: usize,
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
                // `<base href>` aware; the referrer stays the frame's URL.
                Some(v) => url::Url::parse(&crate::loader::node_base_url(&dom, node).unwrap_or_else(|| base.clone()))
                    .ok()
                    .and_then(|b| b.join(v.trim()).ok())
                    .map_or(Source::Invalid, |u| Source::Url(u.to_string())),
            }
        };
        let current = state.images.entries.get(&node).map(|i| (i.url.clone(), i.pending));
        drop(state);
        let url = match source {
            Source::Url(url) if current.as_ref().is_some_and(|(u, _)| *u == url) => {
                // Same request: a `decode()` made since waits for nothing.
                if current.is_some_and(|(_, pending)| !pending) {
                    bindings.settle_image_decodes(node);
                }
                return;
            }
            Source::Url(url) => url,
            Source::None | Source::Invalid => {
                let invalid = matches!(source, Source::Invalid);
                self.doc_mut(page).expect("page").images.entries.remove(&node);
                bindings.set_image_state(node, None, false);
                bindings.keep_loading_image(node, false);
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
            if images.entries.len() >= images.sweep_at {
                let dom = bindings.dom.borrow();
                images.entries.retain(|id, i| i.pending || dom.is_alive(*id));
                images.sweep_at = (images.entries.len() * 2).max(64);
            }
            // Replacing an entry drops its cancellation, stopping the old request.
            images.entries.insert(
                node,
                Image {
                    _node: Some(hold),
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
        // A `srcset`/`<picture>` source without `src` is a request too
        // (`None` would clear the image and reject pending `decode()`s).
        bindings.set_image_state(node, Some(raw_src.as_deref().unwrap_or("")), true);
        bindings.keep_loading_image(node, true);
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
                    refresh: None,
                    referrer_policy: None,
                    timing: None,
                    partial: None,
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
            initiator: Some(lp_webapi::resource_timing::Initiator::Img),
            module: None,
            part: None,
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
        // Nothing needs the element pinned once its request completed.
        let hold = image._node.take();
        drop(state);
        // `decode()` resolves for a parsed header or a format without a
        // header parser; a PNG/GIF/JPEG/WebP whose header does not parse is
        // corrupt.
        let mut decodable = false;
        let mut raster = None;
        let outcome = result.ok().filter(|r| (200..300).contains(&r.status) && !r.body.is_empty()).and_then(|r| {
            let body = &r.body[..r.body.len().min(MAX_IMAGE_BYTES)];
            if let Some((w, h)) = lp_dom::image::dimensions(body) {
                decodable = true;
                raster = Some(r);
                return Some(Some((w as f64, h as f64)));
            }
            if r.mime == "image/svg+xml" || body.trim_ascii_start().starts_with(b"<svg") {
                decodable = true;
                return Some(svg_size(body));
            }
            decodable = !matches!(r.mime.as_str(), "image/png" | "image/gif" | "image/jpeg" | "image/webp");
            r.mime.starts_with("image/").then_some(None)
        });
        let decodable = decodable && outcome.is_some();
        bindings.dom.borrow_mut().set_natural_size(node, outcome.flatten());
        bindings.image_loaded(node, &src, decodable);
        // Canvas drawing decodes the pixels from the bytes, when the
        // document may draw images (it has a canvas or used canvas APIs).
        if let Some(r) = raster.filter(|r| r.body.len() <= MAX_IMAGE_BYTES && bindings.wants_image_bytes()) {
            let clean = self.image_origin_clean(page, &r.url);
            bindings.set_image_bytes(node, r.body, clean);
        }
        // A later owner-thread task: no DOM/state borrow survives dispatch.
        lp_webapi::lifecycle::resource_loaded(bindings, node, outcome.is_some());
        bindings.keep_loading_image(node, false);
        drop(hold);

        self.maybe_fire_load(page);
    }

    /// HTML "origin-clean" for an image response at `url` (no CORS mode:
    /// images load without `crossorigin`): same-origin with the document,
    /// or a `data:`/`blob:` URL.
    fn image_origin_clean(&self, page: FrameKey, url: &str) -> bool {
        if url.starts_with("data:") || url.starts_with("blob:") {
            return true;
        }
        let Some(state) = self.doc(page) else { return false };
        // The document's origin: opaque in a sandboxed frame, inherited by
        // about:blank and about:srcdoc documents.
        let document = &state.main_frame.origin;
        document != "null" && *document == crate::loader::origin_of(url)
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
