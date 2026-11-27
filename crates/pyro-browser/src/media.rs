//! Media element resources for `pyro_webapi::PageHooks::fetch_media`: fetched
//! through the page's resource loader with an Audio or Video destination
//! (CDP resource type "Media"), so request interception and the Network
//! domain see them. Only the container headers are read (`pyro_webapi`'s
//! media module), so the request's `BodyPart` cuts a long or endless body
//! once they are in. A `<video>` keeps the body to decode frames from (pyro-media).
use crate::frames::FrameKey;
use crate::{Browser, FetchRequest, LoaderId, ResourceCancellation, ResourceKind};
use pyro_runtime::TaskSource;
use pyro_webapi::{MediaRequest, MediaResponse};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

pub(crate) type MediaDone = Box<dyn FnOnce(Result<MediaResponse, String>)>;

pub(crate) enum ElementRequest {
    Media(MediaRequest),
    Track { url: String, crossorigin: Option<String> },
}

/// The handle the media element keeps: dropping it cancels the request.
pub(crate) struct MediaFetch {
    pub(crate) cancelled: Rc<Cell<bool>>,
    pub(crate) cancel: Rc<RefCell<Option<ResourceCancellation>>>,
}

impl Drop for MediaFetch {
    fn drop(&mut self) {
        self.cancelled.set(true);
        let cancel = self.cancel.borrow_mut().take();
        drop(cancel);
    }
}

impl Browser {
    /// Starts a media fetch for the document of `page` (`loader`).
    pub(crate) fn fetch_media(
        &mut self,
        page: FrameKey,
        loader: LoaderId,
        request: ElementRequest,
        done: MediaDone,
    ) -> Option<ResourceCancellation> {
        let state = self.doc(page)?;
        let base = state.main_frame.base_url.clone();
        let frame = state.main_frame.clone();
        drop(state);
        let Some(resource_loader) = self.loader_for(page) else {
            self.scheduler().post(TaskSource::Networking, move |_| done(Err("no resource loader".into())));
            return None;
        };
        use pyro_webapi::resource_timing::Initiator;
        let (url, kind, crossorigin, initiator, part) = match request {
            ElementRequest::Media(media) => (
                media.url,
                if media.video { ResourceKind::Video } else { ResourceKind::Audio },
                media.crossorigin,
                if media.video { Initiator::Video } else { Initiator::Audio },
                Some(media.part),
            ),
            ElementRequest::Track { url, crossorigin } => (url, ResourceKind::Track, crossorigin, Initiator::Track, None),
        };
        let track = kind == ResourceKind::Track;
        let cors = crossorigin.is_some();
        if track {
            // Only locally registered blob bytes are eligible. Do not allow
            // an unrecognized scheme to reach a media loader as track data.
            let allowed = url::Url::parse(&url).is_ok_and(|u| match u.scheme() {
                "http" | "https" => cors || (frame.origin != "null" && frame.origin == u.origin().ascii_serialization()),
                "data" => true,
                "blob" => frame.page_dom.as_ref().is_some_and(|d| d.blob_url(&url).is_some()),
                _ => false,
            });
            if !allowed {
                self.scheduler().post(TaskSource::Networking, move |_| done(Err("text track URL not allowed".into())));
                return None;
            }
        }
        let fetch = FetchRequest {
            csp: None,
            required_csp: None,
            purpose: None,
            parser_priority: false,
            credentials: None,
            post: None,
            url,
            kind,
            page: page.page,
            referrer: Some(base),
            referrer_policy: None,
            frame_id: Some(frame.id.0.clone()),
            loader_id: Some(frame.loader_id.0.clone()),
            crossorigin,
            initiator: Some(initiator),
            module: None,
            part,
        };
        resource_loader.fetch_cancellable(
            self,
            fetch,
            Box::new(move |b, result| {
                if b.doc(page).is_some_and(|p| p.main_frame.loader_id == loader) {
                    done(result.and_then(|r| {
                        // CORS-checked (a `crossorigin` request), or same origin.
                        let clean = cors || (!r.opaque && b.image_origin_clean(page, &r.url));
                        if track && (r.opaque || !clean || (r.nosniff && !r.mime.split(';').next().unwrap_or("").trim().eq_ignore_ascii_case("text/vtt"))) {
                            return Err("text track response is not readable".into());
                        }
                        Ok(MediaResponse { status: r.status, mime: r.mime, body: std::sync::Arc::unwrap_or_clone(r.body), partial: r.partial, clean })
                    }));
                }
            }),
        )
    }
}
