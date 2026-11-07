//! Media element resources for `lp_webapi::PageHooks::fetch_media`: fetched
//! whole through the page's resource loader with an Audio or Video
//! destination (CDP resource type "Media"), so request interception and the
//! Network domain see them. Only the container headers are read
//! (`lp_webapi`'s media module); nothing is decoded.
use crate::frames::FrameKey;
use crate::{Browser, FetchRequest, LoaderId, ResourceCancellation, ResourceKind};
use lp_runtime::TaskSource;
use lp_webapi::{MediaRequest, MediaResponse};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

pub(crate) type MediaDone = Box<dyn FnOnce(Result<MediaResponse, String>)>;

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
        request: MediaRequest,
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
        let fetch = FetchRequest {
            post: None,
            url: request.url,
            kind: if request.video { ResourceKind::Video } else { ResourceKind::Audio },
            page: page.page,
            referrer: Some(base),
            frame_id: Some(frame.id.0.clone()),
            loader_id: Some(frame.loader_id.0.clone()),
            crossorigin: request.crossorigin,
            initiator: Some(if request.video {
                lp_webapi::resource_timing::Initiator::Video
            } else {
                lp_webapi::resource_timing::Initiator::Audio
            }),
            module: None,
        };
        resource_loader.fetch_cancellable(
            self,
            fetch,
            Box::new(move |b, result| {
                if b.doc(page).is_some_and(|p| p.main_frame.loader_id == loader) {
                    done(result.map(|r| MediaResponse { status: r.status, mime: r.mime, body: r.body }));
                }
            }),
        )
    }
}
