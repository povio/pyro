//! Diagnostic-only external bundle evaluation with a bounded engine watchdog.
use lp_browser::BrowserThread;
use std::time::{Duration, Instant};
fn main() {
    let path = std::env::args().nth(1).expect("bundle path");
    let source = std::fs::read_to_string(path).expect("bundle source");
    let mut t = BrowserThread::new().unwrap();
    let page = t.browser.create_page().unwrap();
    let realm = t.browser.page(page).unwrap().main_frame.main_realm().unwrap().clone();
    let scope = realm.scope().unwrap();
    let terminate = t.browser.engine.terminate_handle().expect("watchdog capability");
    let (done, recv) = std::sync::mpsc::channel();
    let watchdog = std::thread::spawn(move || {
        if recv.recv_timeout(Duration::from_secs(2)).is_err() {
            terminate.terminate();
        }
    });
    let start = Instant::now();
    let result = scope.eval(&source, Some("exceljs-4.4.0.js"), 1);
    let _ = done.send(());
    watchdog.join().unwrap();
    println!(
        "evaluation_ms={} result={}",
        start.elapsed().as_millis(),
        match &result {
            Ok(_) => "ok".to_owned(),
            Err(e) => format!("{e}"),
        }
    );
    if result.is_err() {
        std::process::exit(1);
    }
    println!(
        "exports={}",
        scope.eval("typeof ExcelJS + ':' + typeof ExcelJS.Workbook", None, 1).unwrap().to_string().unwrap()
    );
}
