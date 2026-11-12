//! `pyro` executable.
//!
//! Commands: `version`, `eval` (JavaScript in an about:blank page until idle),
//! `serve` (CDP server, pyro-cdp) and `fetch` (load a URL over the network, run
//! its scripts, dump the resulting DOM).

use pyro_browser::{BrowserThread, PageEvent, PageId, PageObserver};
use pyro_js::{ConsoleLevel, EngineHooks, JsException, Realm, Value};
use std::cell::Cell;
use std::process::ExitCode;
use std::rc::Rc;
use std::time::{Duration, Instant};

/// The process's Rust heap. macOS's allocator keeps the pages of freed
/// blocks resident and charged to the process (`malloc_zone_pressure_relief`
/// releases none of them): after a closed SPA the system heap held 28 MB
/// for 3 MB of live blocks. mimalloc returns them (when a later allocation
/// finds the purge delay over, at once on [`pyro_js::Engine::settle`]; how,
/// on macOS: `allocator_region`).
#[cfg(not(feature = "heap-profile"))]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[cfg(feature = "heap-profile")]
mod heap_profile;

/// The same heap, sampled (`heap_profile`).
#[cfg(feature = "heap-profile")]
#[global_allocator]
static GLOBAL: heap_profile::Profiled = heap_profile::Profiled(mimalloc::MiMalloc);

unsafe extern "C" {
    /// Frees the calling thread's empty pages (and the blocks other threads
    /// freed into them) and returns the free ones to the system.
    fn mi_collect(force: bool);
}

/// Where mimalloc keeps the Rust heap on macOS, and how its freed pages go
/// back to the system.
///
/// mimalloc purges a free range with `madvise(MADV_FREE_REUSABLE)`. That
/// takes the pages off the process's footprint, but they stay resident
/// until the system wants them: `ps` and Activity Monitor's "Real Memory"
/// went on counting them. Measured after a Playwright run of five SPA page
/// loads on one connection: the heap had 153 MB resident for 59 MB in use
/// (`serve --fork`, the connection's process), 177 MB for 35 MB without
/// `--fork`. So the heap lives in one reserved range handed to mimalloc as
/// an arena (`mi_manage_memory`) whose decommit maps fresh pages over the
/// range: the old ones leave the resident set at once, as JavaScriptCore's
/// do (`JSSetDecommitReplacesPages`). A heap that outgrows the range gets
/// mimalloc's own arenas after it, purged the stock way.
/// `PYRO_HEAP_ARENA=0` keeps mimalloc's own arenas only.
#[cfg(target_os = "macos")]
mod allocator_region {
    use std::ffi::{c_int, c_long, c_void};

    /// mimalloc's `mi_option_os_tag`.
    const MI_OPTION_OS_TAG: c_int = 18;
    /// The VM tag of the heap's regions. mimalloc's default (100) is
    /// `VM_MEMORY_IOACCELERATOR`, so vmmap and footprint showed the Rust
    /// heap as the GPU driver's "IOAccelerator";
    /// `VM_MEMORY_APPLICATION_SPECIFIC_1` shows as "App-Specific Tag 1".
    const VM_MEMORY_APPLICATION_SPECIFIC_1: c_int = 240;
    /// Address space reserved for the heap; pages are used as touched.
    /// mimalloc's bookkeeping for the range is touched in full, though:
    /// 8 GiB cost each process of `serve --fork` 1 MB (idle server 9.5 MB
    /// resident against 8.4 MB), so the range covers what a connection's
    /// pages were measured to need several times over and no more.
    const RESERVE: usize = 2 << 30;

    type CommitFn = unsafe extern "C" fn(bool, *mut c_void, usize, *mut bool, *mut c_void) -> bool;

    unsafe extern "C" {
        fn mi_option_set(option: c_int, value: c_long);
        fn mi_manage_memory(
            start: *mut c_void,
            size: usize,
            is_committed: bool,
            is_pinned: bool,
            is_zero: bool,
            numa_node: c_int,
            exclusive: bool,
            commit_fun: Option<CommitFn>,
            commit_fun_arg: *mut c_void,
            arena_id: *mut *mut c_void,
        ) -> bool;
        fn getenv(name: *const std::ffi::c_char) -> *const std::ffi::c_char;
    }

    /// Anonymous memory carrying the heap's tag (Darwin reads the tag from
    /// the descriptor argument, `VM_MAKE_TAG`).
    unsafe fn map(at: *mut c_void, size: usize, fixed: c_int) -> *mut c_void {
        unsafe {
            libc::mmap(
                at,
                size,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANON | fixed,
                VM_MEMORY_APPLICATION_SPECIFIC_1 << 24,
                0,
            )
        }
    }

    /// mimalloc's commit callback for the arena. Every page of the range
    /// is always usable, so a commit has nothing to do; a decommit swaps
    /// the range for untouched pages.
    unsafe extern "C" fn commit(
        commit: bool,
        start: *mut c_void,
        size: usize,
        is_zero: *mut bool,
        _arg: *mut c_void,
    ) -> bool {
        if commit {
            if !is_zero.is_null() {
                // SAFETY: mimalloc passes null or a valid flag.
                unsafe { *is_zero = false };
            }
            return true;
        }
        // SAFETY: the range lies inside the arena's mapping and mimalloc
        // holds no block in it.
        unsafe { map(start, size, libc::MAP_FIXED) == start }
    }

    extern "C" fn set_up_heap() {
        // SAFETY: mimalloc's public API and libc; nothing else runs yet.
        unsafe {
            mi_option_set(MI_OPTION_OS_TAG, c_long::from(VM_MEMORY_APPLICATION_SPECIFIC_1));
            let off = getenv(c"PYRO_HEAP_ARENA".as_ptr());
            if !off.is_null() && *off == b'0' as std::ffi::c_char {
                return;
            }
            let start = map(std::ptr::null_mut(), RESERVE, 0);
            if start == libc::MAP_FAILED {
                return;
            }
            if !mi_manage_memory(
                start,
                RESERVE,
                false,
                false,
                true,
                -1,
                false,
                Some(commit),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            ) {
                libc::munmap(start, RESERVE);
            }
        }
    }

    /// Runs before `main`, so before the first Rust allocation: mimalloc
    /// makes its first arena at the first allocation and would serve the
    /// heap from that one.
    #[used]
    #[link_section = "__DATA,__mod_init_func"]
    static HEAP_BEFORE_MAIN: extern "C" fn() = set_up_heap;
}

/// Returns mimalloc's free pages to the system now: the browser thread's on
/// [`pyro_js::Engine::settle`], an I/O thread's or the render thread's once it
/// is idle (each owns the pages of what it allocated: response buffers,
/// render documents and images, which the browser thread frees).
fn release_allocator() {
    // SAFETY: mimalloc's public API, callable from any thread at any time.
    unsafe { mi_collect(true) }
}

/// mimalloc's cheap collection: the calling thread's delayed frees and
/// empty pages, without purging what it retains
/// ([`pyro_js::Engine::release_closed`] when the full release waits for idle).
fn trim_allocator() {
    // SAFETY: mimalloc's public API, callable from any thread at any time.
    unsafe { mi_collect(false) }
}

const USAGE: &str = "\
usage: pyro <command> [options]

commands:
  version                      print version and engine capabilities
  eval [--timeout-ms N] (-e CODE | FILE | -)
                               run JavaScript in an about:blank page until idle
  serve [--host H] [--port P]  CDP server (see `serve --help`)
  fetch [--dump html|none] [--wait-ms N] [--timeout-ms N] URL
                               load a page (scripts run), dump the DOM after `load`
";

/// PGO training builds (`scripts/pgo/generate.sh`, `--cfg pyro_pgo_training`) exit
/// normally on SIGTERM/SIGINT so the LLVM profile runtime's atexit hook writes the
/// `.profraw`; the default disposition would kill the process without it. `serve`
/// has no shutdown command, so this is how the training harness stops it.
/// Not compiled into other builds.
#[cfg(pyro_pgo_training)]
fn exit_on_termination_signals() {
    extern "C" fn exit_now(_signal: libc::c_int) {
        std::process::exit(0);
    }
    // SAFETY: installs a plain C handler before any thread exists; `exit`
    // is not async-signal-safe, which a training run accepts.
    let handler = exit_now as *const () as libc::sighandler_t;
    unsafe {
        libc::signal(libc::SIGTERM, handler);
        libc::signal(libc::SIGINT, handler);
    }
}

fn main() -> ExitCode {
    #[cfg(pyro_pgo_training)]
    exit_on_termination_signals();
    #[cfg(feature = "heap-profile")]
    heap_profile::init();
    pyro_js::set_allocator_release(release_allocator);
    pyro_js::set_allocator_trim(trim_allocator);
    pyro_net::set_idle_release(release_allocator);
    #[cfg(feature = "render")]
    pyro_render::worker::set_idle_release(release_allocator);
    pyro_js::configure_process();
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("version") | Some("--version") => version(),
        Some("eval") => eval(&args[1..]),
        Some("serve") => pyro_cdp::serve_main(&args[1..]),
        Some("fetch") => fetch(&args[1..]),
        Some("help") | Some("--help") | Some("-h") | None => {
            print!("{USAGE}");
            ExitCode::SUCCESS
        }
        Some(other) => {
            eprintln!("pyro: unknown command '{other}'\n\n{USAGE}");
            ExitCode::from(2)
        }
    }
}

fn version() -> ExitCode {
    println!("pyro {}", env!("CARGO_PKG_VERSION"));
    let caps = pyro_js::Capabilities::detect();
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
                    eprintln!("pyro: stdin: {e}");
                    return ExitCode::from(2);
                }
                source = Some((s, "stdin.js".into()));
            }
            path => match std::fs::read_to_string(path) {
                Ok(s) => source = Some((s, path.to_string())),
                Err(e) => {
                    eprintln!("pyro: {path}: {e}");
                    return ExitCode::from(2);
                }
            },
        }
    }
    let Some((code, name)) = source else { return usage_error("eval needs -e CODE, FILE or -") };

    let mut thread = match BrowserThread::new() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("pyro: engine init failed: {e}");
            return ExitCode::FAILURE;
        }
    };
    let hooks = Rc::new(CliHooks { failed: Cell::new(false) });
    thread.browser.set_engine_hooks(Some(hooks.clone()));
    thread.browser.set_observer(Some(Rc::new(NoopObserver)));
    let page = match thread.browser.create_page() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("pyro: page creation failed: {e}");
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
            Err(pyro_js::JsError::Terminated) => timed_out = true,
            Err(e) => scope.report_exception(&e),
        }
    }
    if !timed_out {
        timed_out =
            thread.run_until_idle(Some(deadline)) == pyro_runtime::RunOutcome::TimedOut || Instant::now() >= deadline;
    }
    drop(watchdog);
    thread.browser.engine.clear_termination();
    drop(realm);
    thread.browser.close_all();
    if timed_out {
        eprintln!("pyro: timed out after {} ms", timeout.as_millis());
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
    fn start(engine: &pyro_js::Engine, deadline: Instant) -> Option<Self> {
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
    eprintln!("pyro: {msg}\n\n{USAGE}");
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
            eprintln!("pyro: engine init failed: {e}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(e) = thread.enable_network(pyro_net::NetConfig::default()) {
        eprintln!("pyro: network init failed: {e}");
        return ExitCode::FAILURE;
    }
    let hooks = Rc::new(CliHooks { failed: Cell::new(false) });
    let watcher = Rc::new(LoadWatcher::default());
    thread.browser.set_engine_hooks(Some(hooks));
    thread.browser.set_observer(Some(watcher.clone()));
    let page = match thread.browser.create_page() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("pyro: page creation failed: {e}");
            return ExitCode::FAILURE;
        }
    };
    // The initial about:blank load already fired; wait for the real one.
    watcher.loaded.set(false);
    let deadline = Instant::now() + timeout;
    let watchdog = Watchdog::start(&thread.browser.engine, deadline);
    if let Err(e) = thread.browser.navigate(page, pyro_browser::NavigateOpts::address_bar(url.clone())) {
        eprintln!("pyro: navigation failed: {e}");
        return ExitCode::FAILURE;
    }
    let w = watcher.clone();
    let outcome = thread.run_until(Some(deadline), move |_| w.loaded.get() || w.failed.borrow().is_some());
    if let Some(err) = watcher.failed.borrow().clone() {
        eprintln!("pyro: {url}: {err}");
        thread.browser.close_all();
        return ExitCode::FAILURE;
    }
    if outcome == pyro_runtime::RunOutcome::TimedOut || Instant::now() >= deadline {
        drop(watchdog);
        thread.browser.engine.clear_termination();
        eprintln!("pyro: timed out waiting for load of {url}");
        thread.browser.close_all();
        return ExitCode::from(124);
    }
    if !wait.is_zero() {
        thread.run_until_idle(Some((Instant::now() + wait).min(deadline)));
    }
    drop(watchdog);
    thread.browser.engine.clear_termination();
    if Instant::now() >= deadline {
        eprintln!("pyro: timed out after {} ms", timeout.as_millis());
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
            None => eprintln!("pyro: no document to dump"),
        }
    }
    thread.browser.close_all();
    ExitCode::SUCCESS
}
