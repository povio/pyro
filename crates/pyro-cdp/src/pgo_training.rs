//! PGO training builds only (`--cfg pyro_pgo_training`, set by
//! `scripts/pgo/build.sh` together with `-Cprofile-generate`): the calls into
//! the LLVM profile runtime that `serve --fork` needs. The runtime writes a
//! process's counters from an `atexit` hook, which a forked connection's fast
//! exit (`_exit`) and a server that dies of its signal never run, so a
//! training run would record the server and nothing a page did.
//!
//! Every process writes a file of its own. The runtime fixes the name
//! (`LLVM_PROFILE_FILE`, with `%p`) when the server starts, before any fork,
//! so a forked process names its file here. One shared file that every
//! process merges into (`%m`) does not survive a fork-mode run: processes are
//! killed while they write (a retired template 500 ms after its SIGTERM, a
//! harness that stops the process group), and one truncated write loses the
//! whole profile. A truncated file of one process is skipped by the merge.

use std::ffi::{CString, OsString};
use std::os::unix::ffi::OsStringExt;
use std::path::PathBuf;

unsafe extern "C" {
    fn __llvm_profile_write_file() -> libc::c_int;
    fn __llvm_profile_reset_counters();
    fn __llvm_profile_set_filename(name: *const libc::c_char);
}

/// Writes this process's counters to its profile file.
pub(crate) fn write() {
    // SAFETY: the profile runtime is linked into every instrumented build.
    unsafe { __llvm_profile_write_file() };
}

/// In a forked child: forgets the counts inherited from the parent, which
/// writes them itself (every connection would otherwise add the template's
/// start to the profile once more), and gives the child a file of its own
/// next to the parent's.
pub(crate) fn forked() {
    // SAFETY: as in `write`; the child has one thread.
    unsafe { __llvm_profile_reset_counters() };
    let Some(pattern) = std::env::var_os("LLVM_PROFILE_FILE") else { return };
    let dir = PathBuf::from(pattern).parent().map(PathBuf::from).unwrap_or_default();
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    // The time as well: a long run reuses process ids.
    let name = dir.join(format!("fork-{}-{nanos}.profraw", std::process::id()));
    if let Ok(name) = CString::new(OsString::from(name).into_vec()) {
        // SAFETY: a NUL-terminated string, which the runtime copies.
        unsafe { __llvm_profile_set_filename(name.as_ptr()) };
    }
}
