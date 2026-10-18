//! Messages posted from other threads (CDP commands, network completions) run
//! after the page tasks and timers that were ready before they were posted.
use lp_browser::{Browser, BrowserThread, PageId};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn eval(b: &Browser, page: PageId, source: &str) -> String {
    let s = b.page(page).unwrap().main_frame.main_realm().unwrap().scope().unwrap();
    s.eval(source, None, 1).unwrap_or_else(|e| panic!("{}", e.details(&s).text())).to_string().unwrap()
}

#[test]
fn page_work_ready_before_a_remote_message_runs_first() {
    let mut t = BrowserThread::new().unwrap();
    let page = t.browser.create_page().unwrap();
    eval(
        &t.browser,
        page,
        "globalThis.log = [];
         setTimeout(() => log.push('timeout'), 0);
         addEventListener('message', () => log.push('message'));
         postMessage(0, '*');",
    );
    // Both are ready now; a message from another thread arrives afterwards.
    std::thread::sleep(Duration::from_micros(50));
    let seen = Arc::new(Mutex::new(None));
    let (h, seen2) = (t.handle(), seen.clone());
    std::thread::spawn(move || {
        h.post(move |b| *seen2.lock().unwrap() = Some(eval(b, page, "log.slice().sort().join()"))).unwrap()
    })
    .join()
    .unwrap();
    assert_eq!(t.run_until_idle(Some(Instant::now() + Duration::from_secs(2))), lp_runtime::RunOutcome::Idle);
    assert_eq!(seen.lock().unwrap().as_deref(), Some("message,timeout"));
}
