//! Blocking JavaScript dialogs (`alert`, `confirm`, `prompt`, and the
//! `beforeunload` confirmation of `Page.close`).
//!
//! When a session has Page enabled on the page, the dialog suspends the
//! script like Chrome's does: `Page.javascriptDialogOpening` goes out and
//! the connection is served by a nested read loop (the [`DialogPump`] the
//! server installs, `Conn::pause`) until `Page.handleJavaScriptDialog`
//! answers it. No page task or timer runs meanwhile and no CDP state is
//! borrowed: every other command stays queued, in order, and is handled
//! once the dialog closes (Chrome likewise answers renderer commands such
//! as `Runtime.evaluate` only after the dialog). A session detaching or
//! disabling Page, its target closing, or the connection ending dismisses
//! the dialog. Without such a session, without a pump (in-process
//! embedders), or while an `LP.*` driver command is running, dialogs do not
//! block: they are reported and return what a dismissed dialog gives. A
//! page that asked with `LP.dialogs {block: true}` (a client listening for
//! dialogs) blocks during driver commands too.

use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use lp_browser::PageId;
use serde::Deserialize;
use serde_json::json;

use crate::msg::{self, CdpError};
use crate::outbox::Outbox;

/// What a dialog's nested read loop does with one command it read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PumpStep {
    /// Keep the command for the session (it runs after the dialog closes).
    pub hold: bool,
    /// The dialog is over: stop reading.
    pub done: bool,
}

impl PumpStep {
    pub const HOLD: PumpStep = PumpStep { hold: true, done: false };
}

/// Serves the connection while a dialog is showing: offers each command
/// text read (or already queued) to the callback, until it reports `done`
/// (returns true) or the connection ends (returns false).
pub type DialogPump = Rc<dyn Fn(&mut dyn FnMut(&str) -> PumpStep) -> bool>;

/// A page's target id, its browserContextId and its Page-enabled sessions.
type PageSessions = (String, String, Vec<String>);

#[derive(Default)]
pub(crate) struct DialogGate {
    pump: RefCell<Option<DialogPump>>,
    /// Per engine page: its target id, its browserContextId and the
    /// sessions with Page enabled.
    sessions: RefCell<HashMap<PageId, PageSessions>>,
    /// An `LP.*` driver command is running or waiting: dialogs do not block.
    pub(super) driver_busy: Cell<bool>,
    /// A dialog is showing (the nested loop is running).
    open: Cell<bool>,
    /// Pages whose dialogs block even while a driver command runs.
    driver_blocking: RefCell<std::collections::HashSet<PageId>>,
}

#[derive(Deserialize)]
struct Envelope<'a> {
    id: Option<u64>,
    #[serde(borrow)]
    method: Option<Cow<'a, str>>,
    #[serde(rename = "sessionId", borrow)]
    session_id: Option<Cow<'a, str>>,
    #[serde(default)]
    params: serde_json::Value,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct HandleParams {
    accept: bool,
    prompt_text: Option<String>,
}

impl DialogGate {
    pub(super) fn set_pump(&self, pump: Option<DialogPump>) {
        *self.pump.borrow_mut() = pump;
    }

    /// Records the sessions of `page` that have Page enabled.
    pub(super) fn set_sessions(&self, page: PageId, target_id: String, context_id: String, sessions: Vec<String>) {
        let mut map = self.sessions.borrow_mut();
        if sessions.is_empty() {
            map.remove(&page);
        } else {
            map.insert(page, (target_id, context_id, sessions));
        }
    }

    pub(super) fn forget(&self, page: PageId) {
        self.sessions.borrow_mut().remove(&page);
        self.driver_blocking.borrow_mut().remove(&page);
    }

    pub(super) fn set_driver_blocking(&self, page: PageId, block: bool) {
        let mut set = self.driver_blocking.borrow_mut();
        if block {
            set.insert(page);
        } else {
            set.remove(&page);
        }
    }

    /// Whether a dialog of `page` would block now.
    pub(super) fn would_block(&self, page: PageId) -> bool {
        (!self.driver_busy.get() || self.driver_blocking.borrow().contains(&page))
            && !self.open.get()
            && self.pump.borrow().is_some()
            && self.sessions.borrow().contains_key(&page)
    }

    /// Shows a dialog and waits for its answer (see the module docs);
    /// `None` when it does not block.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn show(
        &self,
        out: &Outbox,
        page: PageId,
        frame_id: &str,
        url: &str,
        kind: &str,
        message: &str,
        default_prompt: &str,
    ) -> Option<lp_webapi::DialogAnswer> {
        if !self.would_block(page) {
            return None;
        }
        let pump = self.pump.borrow().clone()?;
        let (target_id, context_id, sessions) = self.sessions.borrow().get(&page).cloned()?;
        self.open.set(true);
        let opening = json!({
            "url": url, "frameId": frame_id, "message": message, "type": kind,
            "hasBrowserHandler": true, "defaultPrompt": default_prompt,
        });
        for s in &sessions {
            out.push(msg::event_frame("Page.javascriptDialogOpening", &opening, Some(s)));
        }
        let mut answer: Option<lp_webapi::DialogAnswer> = None;
        pump(&mut |text: &str| step(text, &target_id, &context_id, &sessions, out, &mut answer));
        self.open.set(false);
        let answer = answer.unwrap_or_default();
        let closed = json!({ "frameId": frame_id, "result": answer.accept, "userInput": answer.prompt_text });
        for s in &sessions {
            out.push(msg::event_frame("Page.javascriptDialogClosed", &closed, Some(s)));
        }
        Some(answer)
    }
}

/// One command read while the dialog shows: answers `handleJavaScriptDialog`
/// from a session of the page; ends the dialog (dismissed) when one of its
/// sessions detaches or disables Page, or its target closes; holds the rest.
fn step(
    text: &str,
    target_id: &str,
    context_id: &str,
    sessions: &[String],
    out: &Outbox,
    answer: &mut Option<lp_webapi::DialogAnswer>,
) -> PumpStep {
    const WATCHED: [&str; 6] = [
        "handleJavaScriptDialog",
        "detachFromTarget",
        "closeTarget",
        "disposeBrowserContext",
        "Page.disable",
        "Page.close",
    ];
    if !WATCHED.iter().any(|m| text.contains(m)) {
        return PumpStep::HOLD;
    }
    let Ok(envelope) = serde_json::from_str::<Envelope<'_>>(text) else { return PumpStep::HOLD };
    let session = envelope.session_id.as_deref();
    let ours = session.is_some_and(|s| sessions.iter().any(|x| x == s));
    let param = |key: &str| envelope.params.get(key).and_then(|v| v.as_str());
    let dismiss = PumpStep { hold: true, done: true };
    match envelope.method.as_deref() {
        Some("Page.handleJavaScriptDialog") if ours => {
            let id = envelope.id;
            match serde_json::from_value::<HandleParams>(envelope.params.clone()) {
                Ok(p) => {
                    *answer = Some(lp_webapi::DialogAnswer {
                        accept: p.accept,
                        prompt_text: p.prompt_text.unwrap_or_default(),
                    });
                    if let Some(id) = id {
                        out.push(msg::result_frame(id, &json!({}), session));
                    }
                    PumpStep { hold: false, done: true }
                }
                Err(e) => {
                    out.push(msg::error_frame(id, &CdpError::invalid_params(e.to_string()), session));
                    PumpStep { hold: false, done: false }
                }
            }
        }
        Some("Page.disable" | "Page.close") if ours => dismiss,
        Some("Target.detachFromTarget")
            if param("sessionId").is_some_and(|s| sessions.iter().any(|x| x == s))
                || param("targetId") == Some(target_id) =>
        {
            dismiss
        }
        Some("Target.closeTarget") if param("targetId") == Some(target_id) => dismiss,
        Some("Target.disposeBrowserContext")
            if !context_id.is_empty() && param("browserContextId") == Some(context_id) =>
        {
            dismiss
        }
        _ => PumpStep::HOLD,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_answer_hold_and_dismiss() {
        let (out, mut rx) = Outbox::channel(crate::outbox::DEFAULT_OUTBOUND_BUDGET);
        let sessions = vec!["S1".to_owned()];
        let mut answer = None;
        {
            let mut run = |t: &str| step(t, "T1", "C1", &sessions, &out, &mut answer);
            assert_eq!(run(r#"{"id":1,"method":"Runtime.evaluate","sessionId":"S1","params":{}}"#), PumpStep::HOLD);
            assert_eq!(
                run(r#"{"id":2,"method":"Page.handleJavaScriptDialog","sessionId":"OTHER","params":{"accept":true}}"#),
                PumpStep::HOLD
            );
            assert_eq!(
                run(r#"{"id":3,"method":"Target.closeTarget","params":{"targetId":"T1"}}"#),
                PumpStep { hold: true, done: true }
            );
            assert_eq!(
                run(r#"{"id":3,"method":"Target.disposeBrowserContext","params":{"browserContextId":"C2"}}"#),
                PumpStep::HOLD
            );
            assert_eq!(
                run(r#"{"id":3,"method":"Target.disposeBrowserContext","params":{"browserContextId":"C1"}}"#),
                PumpStep { hold: true, done: true }
            );
            assert_eq!(
                run(r#"{"id":4,"method":"Page.handleJavaScriptDialog","sessionId":"S1","params":{}}"#),
                PumpStep { hold: false, done: false },
                "bad params are answered with an error"
            );
            assert_eq!(
                run(
                    r#"{"id":5,"method":"Page.handleJavaScriptDialog","sessionId":"S1","params":{"accept":true,"promptText":"x"}}"#
                ),
                PumpStep { hold: false, done: true }
            );
        }
        assert_eq!(answer, Some(lp_webapi::DialogAnswer { accept: true, prompt_text: "x".into() }));
        let mut frames = 0;
        while rx.try_recv().is_some() {
            frames += 1;
        }
        assert_eq!(frames, 2, "an error for #4 and the reply to #5");
    }
}
