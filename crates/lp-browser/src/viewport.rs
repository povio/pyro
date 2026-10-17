//! Per-page output metrics and document-scoped resize delivery.
use crate::{Browser, Page, PageId};
use lp_dom::media::Viewport;
use lp_js::JsError;
use lp_runtime::TaskSource;
use lp_webapi::event::{EventCore, EventInterface};

impl Page {
    /// Persistent metrics, including media/input preferences. Clone and update
    /// selected fields when changing metrics without resetting other emulation.
    pub fn viewport(&self) -> &Viewport {
        &self.viewport
    }
}

impl Browser {
    /// Updates this page's current and future document metrics. A changed CSS
    /// width/height schedules a coalesced trusted window resize at a safe point.
    /// Call outside DOM/layout borrows. Rejects invalid DPR without changing state.
    pub fn set_viewport(&mut self, page: PageId, viewport: Viewport) -> Result<(), JsError> {
        let state = self.page_mut(page).ok_or_else(|| JsError::Type("no such page".into()))?;
        if !viewport.device_scale_factor.is_finite() || viewport.device_scale_factor <= 0.0 {
            return Err(JsError::Range("device scale factor must be finite and positive".into()));
        }
        if state.viewport == viewport {
            return Ok(());
        }
        let old_size = (state.viewport.width, state.viewport.height);
        if let Some(bindings) = &state.main_frame.page_dom {
            bindings.set_viewport(viewport.clone());
        }
        state.viewport = viewport;
        let loader = state.main_frame.loader_id.clone();
        if state.pending_resize.as_ref() == Some(&loader) {
            return Ok(());
        }
        state.pending_resize = Some(loader.clone());
        self.scheduler().post(TaskSource::DomManipulation, move |browser| {
            let Some(state) = browser.page_mut(page) else { return };
            if state.pending_resize.as_ref() != Some(&loader) {
                return;
            }
            state.pending_resize = None;
            if state.main_frame.loader_id != loader {
                return;
            }
            let Some(bindings) = state.main_frame.page_dom.clone() else { return };
            // No browser/DOM/layout borrow crosses JavaScript listener invocation.
            if (state.viewport.width, state.viewport.height) != old_size {
                if let Some(window) = bindings.window_target() {
                    let event = EventCore::trusted(&bindings, EventInterface::Event, "resize", false, false);
                    lp_webapi::target::fire(&bindings, window, &event);
                }
            }
            // Media matching can stop a pending sheet from blocking a parser
            // script even when CSS width/height did not change.
            browser.stylesheets_changed(page);
        });
        Ok(())
    }
}
