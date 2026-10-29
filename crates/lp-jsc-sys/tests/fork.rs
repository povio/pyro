//! fork() support of the patched JavaScriptCore (`webkit/patches/0002`): a
//! child keeps using the parent's VM, with its state, after JSPrepareForFork.
//! Skipped on a JavaScriptCore without it.

use lp_jsc_sys::*;
use std::ffi::CString;

unsafe extern "C" {
    fn fork() -> i32;
    fn waitpid(pid: i32, status: *mut i32, options: i32) -> i32;
    fn _exit(code: i32) -> !;
}

unsafe fn eval(ctx: JSGlobalContextRef, src: &str) -> String {
    unsafe {
        let src = CString::new(src).unwrap();
        let script = JSStringCreateWithUTF8CString(src.as_ptr());
        let mut exc: JSValueRef = core::ptr::null();
        let v = JSEvaluateScript(ctx, script, core::ptr::null_mut(), core::ptr::null_mut(), 1, &mut exc);
        JSStringRelease(script);
        let s = JSValueToStringCopy(ctx, if exc.is_null() { v } else { exc }, core::ptr::null_mut());
        let mut buf = vec![0u8; JSStringGetMaximumUTF8CStringSize(s)];
        let n = JSStringGetUTF8CString(s, buf.as_mut_ptr().cast(), buf.len());
        JSStringRelease(s);
        String::from_utf8_lossy(&buf[..n.saturating_sub(1)]).into_owned()
    }
}

#[test]
fn child_continues_from_the_parents_heap() {
    let Some((prepare, did_fork, reseed)) = spi::get().fork() else {
        eprintln!("skipped: this JavaScriptCore cannot fork");
        return;
    };
    unsafe {
        let group = JSContextGroupCreate();
        let ctx = JSGlobalContextCreateInGroup(group, core::ptr::null_mut());
        // Enough code and garbage that the JIT and the GC threads have run.
        eval(
            ctx,
            "globalThis.app = { fns: [] }; \
             for (let m = 0; m < 2000; m++) app.fns.push(new Function('x', 'return x + ' + m)); \
             function hot(n) { let s = 0; for (let i = 0; i < n; i++) s = (s + app.fns[i % 2000](i)) | 0; return s; } \
             hot(2e6); Math.random()",
        );
        let random_file = std::env::temp_dir().join(format!("lp-jsc-fork-{}", std::process::id()));
        for _ in 0..2 {
            assert!(prepare(group, 5.0), "helper threads did not stop");
            let pid = fork();
            if pid == 0 {
                did_fork(true);
                reseed(ctx);
                let ok = eval(
                    ctx,
                    "let g = []; for (let i = 0; i < 2e5; i++) g.push({ i }); g = null; \
                     app.fns.push(1); [app.fns.length, typeof hot(3e6)].join()",
                ) == "2001,number";
                std::fs::write(&random_file, eval(ctx, "Math.random()")).unwrap();
                JSGarbageCollect(ctx);
                _exit(if ok { 0 } else { 1 });
            }
            did_fork(false);
            let mut status = -1;
            waitpid(pid, &mut status, 0);
            assert_eq!(status, 0, "child failed (status {status})");
            assert_eq!(eval(ctx, "app.fns.length"), "2000", "the child's writes stay in the child");
            // Without the reseed the child would draw exactly the parent's next number.
            assert_ne!(std::fs::read_to_string(&random_file).unwrap(), eval(ctx, "Math.random()"));
        }
        let _ = std::fs::remove_file(&random_file);
        JSGlobalContextRelease(ctx);
        JSContextGroupRelease(group);
    }
}
