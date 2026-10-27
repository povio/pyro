//! Shipped-runtime integration (diagnostic, macOS): JavaScriptCore's own GC
//! timers live on the run loop of the thread that created the VM; the browser
//! thread's loop services that run loop at a bounded interval, so wrappers of
//! a closed page are finalized while lp-runtime has nothing else to do.
//! Timing depends on JavaScriptCore's natural GC scheduling, so the test is
//! ignored in ordinary runs and executed explicitly:
//! `cargo test -p lp-browser --test integration -- runloop_idle_finalization:: --ignored --nocapture`.
//! On the baseline (no service) it times out: that is the discriminator.
#![cfg(target_os = "macos")]
use lp_browser::{BrowserThread, PageId};
use lp_runtime::RunOutcome;
use std::time::{Duration, Instant};

fn eval(thread: &BrowserThread, page: PageId, script: &str) -> String {
    thread
        .browser
        .page(page)
        .unwrap()
        .main_frame
        .main_realm()
        .unwrap()
        .scope()
        .unwrap()
        .eval(script, None, 1)
        .unwrap()
        .to_string()
        .unwrap()
}

#[test]
#[ignore = "shipped-runtime integration; depends on JavaScriptCore's natural GC timing"]
fn closed_page_wrappers_are_finalized_while_the_loop_idles() {
    let mut thread = BrowserThread::new().unwrap();
    let page = thread.browser.create_page().unwrap();
    let count = eval(&thread, page, "for (let i = 0; i < 50000; i++) { const d = document.createElement('div'); d.textContent = 'n' + i; d.onclick = () => i; document.body.appendChild(d); } document.body.childElementCount");
    assert_eq!(count, "50000");
    thread.run_until_idle(Some(Instant::now() + Duration::from_secs(1)));
    assert!(thread.browser.close_page(page));
    thread.browser.checkpoint();
    let after_close = *thread.browser.released_payloads.borrow();
    let start = Instant::now();
    let outcome =
        thread.run_until(Some(start + Duration::from_secs(10)), |b| *b.released_payloads.borrow() > after_close);
    let after_idle = *thread.browser.released_payloads.borrow();
    eprintln!(
        "released payloads: after close {after_close}, after idle {after_idle} (+{}), outcome {outcome:?} after {:?}",
        after_idle - after_close,
        start.elapsed()
    );
    assert_eq!(outcome, RunOutcome::Done, "no wrapper was finalized during a 10 s idle wait");
}
