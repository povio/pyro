//! `lightningpanda` executable.
//!
//! Implemented: `version`, `eval` (runs JavaScript on real JavaScriptCore in a
//! page realm with console and timers, until the event loop is idle).
//! Not yet implemented (exit code 2 with a message): `serve` (CDP, worker 3)
//! and `fetch` (document loading, worker 2).

use lp_browser::{BrowserThread, PageEvent, PageId, PageObserver};
use lp_js::{ConsoleLevel, EngineHooks, JsException, Realm, Value};
use std::cell::Cell;
use std::process::ExitCode;
use std::rc::Rc;
use std::time::{Duration, Instant};

const USAGE: &str = "\
usage: lightningpanda <command> [options]

commands:
  version                      print version and engine capabilities
  eval [--timeout-ms N] (-e CODE | FILE | -)
                               run JavaScript in an about:blank page until idle
  serve [--host H] [--port P]  CDP server (not implemented yet)
  fetch URL                    load a page and dump it (not implemented yet)
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("version") | Some("--version") => version(),
        Some("eval") => eval(&args[1..]),
        Some("serve") | Some("fetch") => {
            eprintln!("lightningpanda: '{}' is not implemented yet in the Rust rewrite", args[0]);
            ExitCode::from(2)
        }
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
    println!("engine: system JavaScriptCore; capabilities: {caps:?}");
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
    {
        let scope = realm.scope().expect("live realm");
        if let Err(e) = scope.eval(&code, Some(&name), 1) {
            scope.report_exception(&e);
        }
    }
    let deadline = Instant::now() + timeout;
    let outcome = thread.run_until_idle(Some(deadline));
    thread.browser.close_all();
    if outcome == lp_runtime::RunOutcome::TimedOut {
        eprintln!("lightningpanda: timed out after {} ms with pending work", timeout.as_millis());
        return ExitCode::from(124);
    }
    if hooks.failed.get() {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

fn usage_error(msg: &str) -> ExitCode {
    eprintln!("lightningpanda: {msg}\n\n{USAGE}");
    ExitCode::from(2)
}
