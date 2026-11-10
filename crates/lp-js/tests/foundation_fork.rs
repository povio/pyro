#![cfg(target_os = "macos")]
//! Fresh-process native proof: parent threads stay alive across the fork, and
//! the child executes real Foundation number operations rather than a once guard.

use std::process::Command;
use std::sync::mpsc;

unsafe extern "C" {
    fn fork() -> i32;
    fn waitpid(pid: i32, status: *mut i32, options: i32) -> i32;
    fn _exit(code: i32) -> !;
}

#[test]
fn foundation_numbers_are_initialized_before_the_first_fork() {
    const PROBE: &str = "LP_FOUNDATION_FORK_TEST_CHILD";
    if std::env::var_os(PROBE).is_none() {
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "foundation_fork::foundation_numbers_are_initialized_before_the_first_fork",
                "--nocapture",
            ])
            .env(PROBE, "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "fresh-process fork probe failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    // No engine/VM or network exists in this fresh process. Warm in the parent;
    // the child must not be the first process to initialize the classes.
    lp_js::prepare_process_for_fork();
    let (ready_tx, ready_rx) = mpsc::sync_channel(0);
    let (stop_tx, stop_rx) = mpsc::sync_channel(0);
    let background = std::thread::spawn(move || {
        ready_tx.send(()).unwrap();
        stop_rx.recv().unwrap();
    });
    ready_rx.recv().unwrap();
    // SAFETY: this isolated probe deliberately keeps a waiting background
    // thread alive. The child touches only the warmed native number classes
    // and terminates with _exit, avoiding inherited Rust allocator/lock state.
    let pid = unsafe { fork() };
    assert!(pid >= 0, "fork failed");
    if pid == 0 {
        let valid = unsafe { lp_jsc_sys::foundation::lp_foundation_initialize_numbers() };
        unsafe { _exit(if valid == 1 { 0 } else { 1 }) };
    }
    let mut status = -1;
    let waited = unsafe { waitpid(pid, &mut status, 0) };
    stop_tx.send(()).unwrap();
    background.join().unwrap();
    assert_eq!(waited, pid);
    assert_eq!(status, 0, "child failed while using Foundation numbers: {status}");
}
