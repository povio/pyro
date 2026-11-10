//! Bound Objective-C temporaries to a synchronous JSC entry, including the
//! microtasks it drains. Module work reached through the C API can autorelease
//! JSContext/JSVirtualMachine wrappers even outside the Objective-C shim. Those
//! wrappers retain the VM: leaving them in the thread's fallback pool postpones
//! finalizers until after EngineInner and its native finalization queue are gone.

use std::os::raw::c_void;

/// Keep this guard local to one call frame so pools are always popped in LIFO
/// order, including on Rust unwinding. Never store it in a realm or engine:
/// their lifetimes can overlap without being nested.
#[cfg(target_os = "macos")]
pub(crate) struct Pool(*mut c_void);

/// No Objective-C runtime elsewhere: nothing autoreleases.
#[cfg(not(target_os = "macos"))]
pub(crate) struct Pool(std::marker::PhantomData<*mut c_void>);

#[cfg(not(target_os = "macos"))]
impl Pool {
    pub(crate) fn new() -> Self {
        Self(std::marker::PhantomData)
    }
}

#[cfg(target_os = "macos")]
impl Pool {
    pub(crate) fn new() -> Self {
        // SAFETY: paired with Drop on this thread; the raw pointer is !Send.
        Self(unsafe { lp_jsc_sys::autorelease::objc_autoreleasePoolPush() })
    }
}

#[cfg(target_os = "macos")]
impl Drop for Pool {
    fn drop(&mut self) {
        // SAFETY: the private guard stays in the synchronous frame that pushed it.
        unsafe { lp_jsc_sys::autorelease::objc_autoreleasePoolPop(self.0) };
    }
}
