//! `lightningpanda` executable.
//!
//! Commands: `version`, `eval` (JavaScript in an about:blank page until idle),
//! `serve` (CDP server, lp-cdp) and `fetch` (load a URL over the network, run
//! its scripts, dump the resulting DOM).

use lp_browser::{BrowserThread, PageEvent, PageId, PageObserver};
use lp_js::{ConsoleLevel, EngineHooks, JsException, Realm, Value};
use std::cell::Cell;
use std::process::ExitCode;
use std::rc::Rc;
use std::time::{Duration, Instant};

/// The process's Rust heap. macOS's allocator keeps the pages of freed
/// blocks resident and charged to the process (`malloc_zone_pressure_relief`
/// releases none of them): after a closed SPA the system heap held 28 MB
/// for 3 MB of live blocks. mimalloc returns them (`MADV_FREE_REUSABLE`
/// after a delay, at once on [`lp_js::Engine::settle`]).
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

unsafe extern "C" {
    /// Frees the calling thread's empty pages (and the blocks other threads
    /// freed into them) and returns the free ones to the system.
    fn mi_collect(force: bool);
}

/// The VM tag of mimalloc's regions. Its default (100) is
/// `VM_MEMORY_IOACCELERATOR`, so vmmap and footprint showed the Rust heap as
/// the GPU driver's "IOAccelerator"; `VM_MEMORY_APPLICATION_SPECIFIC_1`
/// shows as "App-Specific Tag 1".
#[cfg(target_os = "macos")]
mod allocator_tag {
    use std::ffi::{c_int, c_long};

    /// mimalloc's `mi_option_os_tag`.
    const MI_OPTION_OS_TAG: c_int = 18;
    const VM_MEMORY_APPLICATION_SPECIFIC_1: c_long = 240;

    unsafe extern "C" {
        fn mi_option_set(option: c_int, value: c_long);
    }

    extern "C" fn tag_allocator_regions() {
        // SAFETY: mimalloc's public API; it only stores the value.
        unsafe { mi_option_set(MI_OPTION_OS_TAG, VM_MEMORY_APPLICATION_SPECIFIC_1) }
    }

    /// Runs before `main`, so before the first Rust allocation: mimalloc
    /// tags a region when it maps it, and its first arena serves the heap
    /// from then on.
    #[used]
    #[link_section = "__DATA,__mod_init_func"]
    static TAG_BEFORE_MAIN: extern "C" fn() = tag_allocator_regions;
}

/// Returns mimalloc's free pages to the system now: the browser thread's on
/// [`lp_js::Engine::settle`], an I/O thread's once it is idle (it owns the
/// response buffers the browser thread frees).
fn release_allocator() {
    // SAFETY: mimalloc's public API, callable from any thread at any time.
    unsafe { mi_collect(true) }
}

const USAGE: &str = "\
usage: lightningpanda <command> [options]

commands:
  version                      print version and engine capabilities
  eval [--timeout-ms N] (-e CODE | FILE | -)
                               run JavaScript in an about:blank page until idle
  serve [--host H] [--port P]  CDP server (see `serve --help`)
  fetch [--dump html|none] [--wait-ms N] [--timeout-ms N] URL
                               load a page (scripts run), dump the DOM after `load`
";

fn main() -> ExitCode {
    lp_js::set_allocator_release(release_allocator);
    lp_net::set_idle_release(release_allocator);
    lp_js::configure_process();
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("version") | Some("--version") => version(),
        Some("eval") => eval(&args[1..]),
        Some("serve") => lp_cdp::serve_main(&args[1..]),
        Some("fetch") => fetch(&args[1..]),
        Some("help") | Some("--help") | Some("-h") | None => {
            print!("{USAGE}");
            ExitCode::SUCCESS
        }
        Some(other) => {
            eprintln!("lightningpanda: unknown command '{other}'\n\n{USAGE}");
            ExitCode::from(2)
        }
    }
}

fn version() -> ExitCode {
    println!("lightningpanda {}", env!("CARGO_PKG_VERSION"));
    let caps = lp_js::Capabilities::detect();
    println!("engine: JavaScriptCore; capabilities: {caps:?}");
    ExitCode::SUCCESS
}

struct CliHooks {
    failed: Cell<bool>,
}

fn render(args: &[Value<'_>]) -> String {
    args.iter()
        .map(|a| {
            if a.is_string() {
                a.to_string().unwrap_or_default()
            } else if a.is_object() && !a.is_function() {
                a.to_json(0).ok().flatten().unwrap_or_else(|| a.to_string().unwrap_or_default())
            } else {
                a.to_string().unwrap_or_default()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

impl EngineHooks for CliHooks {
    fn console_api(&self, _realm: &Realm, level: ConsoleLevel, args: &[Value<'_>]) {
        match level {
            ConsoleLevel::Error | ConsoleLevel::Warning | ConsoleLevel::Assert => {
                eprintln!("{}", render(args))
            }
            _ => println!("{}", render(args)),
        }
    }
    fn exception_thrown(&self, _realm: &Realm, e: &JsException) {
        self.failed.set(true);
        eprintln!("Uncaught {}", e.text());
        if let Some(stack) = &e.stack {
            eprintln!("{stack}");
        }
    }
    fn unhandled_rejection(&self, _realm: &Realm, e: &JsException) {
        self.failed.set(true);
        eprintln!("Unhandled promise rejection: {}", e.text());
    }
}

struct NoopObserver;
impl PageObserver for NoopObserver {
    fn on_page_event(&self, _page: PageId, _event: &PageEvent) {}
}

fn eval(args: &[String]) -> ExitCode {
    let mut timeout = Duration::from_secs(30);
    let mut source: Option<(String, String)> = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--timeout-ms" => match it.next().and_then(|v| v.parse().ok()) {
                Some(ms) => timeout = Duration::from_millis(ms),
                None => return usage_error("--timeout-ms needs a number"),
            },
            "-e" => match it.next() {
                Some(code) => source = Some((code.clone(), "eval.js".into())),
                None => return usage_error("-e needs code"),
            },
            "-" => {
                let mut s = String::new();
                if let Err(e) = std::io::Read::read_to_string(&mut std::io::stdin(), &mut s) {
                    eprintln!("lightningpanda: stdin: {e}");
                    return ExitCode::from(2);
                }
                source = Some((s, "stdin.js".into()));
            }
            path => match std::fs::read_to_string(path) {
                Ok(s) => source = Some((s, path.to_string())),
                Err(e) => {
                    eprintln!("lightningpanda: {path}: {e}");
                    return ExitCode::from(2);
                }
            },
        }
    }
    let Some((code, name)) = source else { return usage_error("eval needs -e CODE, FILE or -") };

    let mut thread = match BrowserThread::new() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("lightningpanda: engine init failed: {e}");
            return ExitCode::FAILURE;
        }
    };
    let hooks = Rc::new(CliHooks { failed: Cell::new(false) });
    thread.browser.set_engine_hooks(Some(hooks.clone()));
    thread.browser.set_observer(Some(Rc::new(NoopObserver)));
    let page = match thread.browser.create_page() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("lightningpanda: page creation failed: {e}");
            return ExitCode::FAILURE;
        }
    };
    let realm = thread.browser.page(page).and_then(|p| p.main_frame.main_realm()).cloned().expect("main realm");
    // The deadline covers the initial evaluation too: a watchdog thread asks
    // the engine to terminate running JavaScript once it passes.
    let deadline = Instant::now() + timeout;
    let watchdog = Watchdog::start(&thread.browser.engine, deadline);
    let mut timed_out = false;
    {
        let scope = realm.scope().expect("live realm");
        match scope.eval(&code, Some(&name), 1) {
            Ok(_) => {}
            Err(lp_js::JsError::Terminated) => timed_out = true,
            Err(e) => scope.report_exception(&e),
        }
    }
    if !timed_out {
        timed_out =
            thread.run_until_idle(Some(deadline)) == lp_runtime::RunOutcome::TimedOut || Instant::now() >= deadline;
    }
    drop(watchdog);
    thread.browser.engine.clear_termination();
    drop(realm);
    thread.browser.close_all();
    if timed_out {
        eprintln!("lightningpanda: timed out after {} ms", timeout.as_millis());
        return ExitCode::from(124);
    }
    if hooks.failed.get() {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// Interrupts JavaScript at the command deadline, including inside task callbacks.
/// Drop always stops and joins the thread before the engine is torn down.
struct Watchdog {
    stop: std::sync::mpsc::Sender<()>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Watchdog {
    fn start(engine: &lp_js::Engine, deadline: Instant) -> Option<Self> {
        let handle = engine.terminate_handle().ok()?;
        let (stop, rx) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || {
            if rx.recv_timeout(deadline.saturating_duration_since(Instant::now()))
                == Err(std::sync::mpsc::RecvTimeoutError::Timeout)
            {
                loop {
                    handle.terminate();
                    // A caught termination inside a native frame may need another request.
                    if rx.recv_timeout(Duration::from_millis(50)) != Err(std::sync::mpsc::RecvTimeoutError::Timeout) {
                        break;
                    }
                }
            }
        });
        Some(Self { stop, thread: Some(thread) })
    }
}
impl Drop for Watchdog {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn usage_error(msg: &str) -> ExitCode {
    eprintln!("lightningpanda: {msg}\n\n{USAGE}");
    ExitCode::from(2)
}

#[derive(Default)]
struct LoadWatcher {
    loaded: Cell<bool>,
    failed: std::cell::RefCell<Option<String>>,
}

impl PageObserver for LoadWatcher {
    fn on_page_event(&self, _page: PageId, event: &PageEvent) {
        match event {
            PageEvent::FrameLoaded { .. } => self.loaded.set(true),
            PageEvent::FrameNavigateFailed { error, .. } => *self.failed.borrow_mut() = Some(error.clone()),
            _ => {}
        }
    }
}

fn fetch(args: &[String]) -> ExitCode {
    let mut dump_html = true;
    let mut wait = Duration::ZERO;
    let mut timeout = Duration::from_secs(30);
    let mut url: Option<String> = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--dump" => match it.next().map(String::as_str) {
                Some("html") => dump_html = true,
                Some("none") => dump_html = false,
                _ => return usage_error("--dump takes html or none"),
            },
            "--wait-ms" => match it.next().and_then(|v| v.parse().ok()) {
                Some(ms) => wait = Duration::from_millis(ms),
                None => return usage_error("--wait-ms needs a number"),
            },
            "--timeout-ms" => match it.next().and_then(|v| v.parse().ok()) {
                Some(ms) => timeout = Duration::from_millis(ms),
                None => return usage_error("--timeout-ms needs a number"),
            },
            u if !u.starts_with('-') && url.is_none() => url = Some(u.to_string()),
            other => return usage_error(&format!("unexpected argument {other}")),
        }
    }
    let Some(url) = url else { return usage_error("fetch needs a URL") };
    let mut thread = match BrowserThread::new() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("lightningpanda: engine init failed: {e}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(e) = thread.enable_network(lp_net::NetConfig::default()) {
        eprintln!("lightningpanda: network init failed: {e}");
        return ExitCode::FAILURE;
    }
    let hooks = Rc::new(CliHooks { failed: Cell::new(false) });
    let watcher = Rc::new(LoadWatcher::default());
    thread.browser.set_engine_hooks(Some(hooks));
    thread.browser.set_observer(Some(watcher.clone()));
    let page = match thread.browser.create_page() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("lightningpanda: page creation failed: {e}");
            return ExitCode::FAILURE;
        }
    };
    // The initial about:blank load already fired; wait for the real one.
    watcher.loaded.set(false);
    let deadline = Instant::now() + timeout;
    let watchdog = Watchdog::start(&thread.browser.engine, deadline);
    if let Err(e) = thread.browser.navigate(page, lp_browser::NavigateOpts::address_bar(url.clone())) {
        eprintln!("lightningpanda: navigation failed: {e}");
        return ExitCode::FAILURE;
    }
    let w = watcher.clone();
    let outcome = thread.run_until(Some(deadline), move |_| w.loaded.get() || w.failed.borrow().is_some());
    if let Some(err) = watcher.failed.borrow().clone() {
        eprintln!("lightningpanda: {url}: {err}");
        thread.browser.close_all();
        return ExitCode::FAILURE;
    }
    if outcome == lp_runtime::RunOutcome::TimedOut || Instant::now() >= deadline {
        drop(watchdog);
        thread.browser.engine.clear_termination();
        eprintln!("lightningpanda: timed out waiting for load of {url}");
        thread.browser.close_all();
        return ExitCode::from(124);
    }
    if !wait.is_zero() {
        thread.run_until_idle(Some((Instant::now() + wait).min(deadline)));
    }
    drop(watchdog);
    thread.browser.engine.clear_termination();
    if Instant::now() >= deadline {
        eprintln!("lightningpanda: timed out after {} ms", timeout.as_millis());
        thread.browser.close_all();
        return ExitCode::from(124);
    }
    if dump_html {
        let html = thread.browser.page(page).and_then(|p| {
            let dom = p.main_frame.dom.as_ref()?;
            let doc = p.main_frame.document?;
            let d = dom.borrow();
            let mut out = String::new();
            if d.doctype(doc).ok().flatten().is_some() {
                out.push_str("<!DOCTYPE html>\n");
            }
            let root = d.document_element(doc).ok().flatten()?;
            out.push_str(&d.outer_html(root).ok()?);
            Some(out)
        });
        match html {
            Some(h) => println!("{h}"),
            None => eprintln!("lightningpanda: no document to dump"),
        }
    }
    thread.browser.close_all();
    ExitCode::SUCCESS
}
