//! Per-page output metrics and document-scoped resize delivery.
use crate::{Browser, Page, PageId};
use pyro_dom::media::Viewport;
use pyro_js::JsError;
use pyro_runtime::TaskSource;
use pyro_webapi::event::{EventCore, EventInterface};

impl Page {
    /// Native window dimensions used when a CDP device metrics override is cleared.
    pub fn native_viewport(&self) -> &Viewport {
        &self.native_viewport
    }

    /// Persistent metrics, including media/input preferences. Clone and update
    /// selected fields when changing metrics without resetting other emulation.
    pub fn viewport(&self) -> &Viewport {
        &self.viewport
    }
}

impl Browser {
    /// Updates this page's current and future document metrics. A changed CSS
    /// width/height (under mobile emulation: layout viewport, window or
    /// visual viewport) schedules a coalesced trusted window (and visual
    /// viewport) resize at a safe point. Call outside DOM/layout borrows.
    /// Rejects invalid DPR without changing state.
    pub fn set_viewport(&mut self, page: PageId, viewport: Viewport) -> Result<(), JsError> {
        let state = self.page_mut(page).ok_or_else(|| JsError::Type("no such page".into()))?;
        if !viewport.device_scale_factor.is_finite() || viewport.device_scale_factor <= 0.0 {
            return Err(JsError::Range("device scale factor must be finite and positive".into()));
        }
        if state.viewport == viewport {
            return Ok(());
        }
        let old_metrics = match &state.main_frame.page_dom {
            Some(bindings) => bindings.layout().viewport_metrics(),
            None => {
                let (w, h) = (state.viewport.width, state.viewport.height);
                pyro_dom::layout::ViewportMetrics { layout: (w, h), window: (w, h), visual: (w as f64, h as f64, 1.0) }
            }
        };
        let old_orientation = state.main_frame.page_dom.as_ref().map(|b| pyro_webapi::window::screen_orientation(b));
        if let Some(bindings) = &state.main_frame.page_dom {
            bindings.set_viewport(viewport.clone());
        }
        state.viewport = viewport;
        let loader = state.main_frame.loader_id.clone();
        if state.pending_resize.as_ref() == Some(&loader) {
            return Ok(());
        }
        state.pending_resize = Some(loader.clone());
        let old_orientation = old_orientation.unwrap_or_default();
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
            // The layout viewport, the window and the visual viewport are the
            // emulated viewport, unless it is mobile: then the window resizes
            // with the layout viewport or the frame, the visual viewport with
            // its size or page scale.
            bindings.sync_mobile_layout();
            let now = bindings.layout().viewport_metrics();
            // No browser/DOM/layout borrow crosses JavaScript listener invocation.
            if now.window_resized(&old_metrics) {
                if let Some(window) = bindings.window_target() {
                    let event = EventCore::trusted(&bindings, EventInterface::Event, "resize", false, false);
                    pyro_webapi::target::fire(&bindings, window, &event);
                }
            }
            if now.visual_resized(&old_metrics) {
                for target in bindings.viewport_targets(pyro_webapi::window::ViewportTarget::VisualViewport) {
                    let event = EventCore::trusted(&bindings, EventInterface::Event, "resize", false, false);
                    pyro_webapi::target::fire(&bindings, target, &event);
                }
            }
            if pyro_webapi::window::screen_orientation(&bindings) != old_orientation {
                for target in bindings.viewport_targets(pyro_webapi::window::ViewportTarget::Orientation) {
                    let event = EventCore::trusted(&bindings, EventInterface::Event, "change", false, false);
                    pyro_webapi::target::fire(&bindings, target, &event);
                }
            }
            // Media query lists whose `matches` flipped (size, color scheme,
            // reduced motion, print) get `change`.
            for (target, query, matches) in bindings.media_query_changes() {
                bindings.fire_media_query_change(target, &query, matches);
            }
            // Child frames sized from their iframes: one that changed size
            // gets its own `resize` task.
            if let Some(host) = browser.page(page).map(|p| p.frames.clone()).filter(|h| h.has_children()) {
                for key in host.keys().into_iter().filter(|k| k.child.is_some()) {
                    if let Some(child) = host.page_dom(key) {
                        child.sync_frame_viewport();
                    }
                }
            }
            // Media matching can stop a pending sheet from blocking a parser
            // script even when CSS width/height did not change.
            browser.stylesheets_changed(page.into());
        });
        Ok(())
    }
}
