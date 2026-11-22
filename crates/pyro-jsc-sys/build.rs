// Links JavaScriptCore on macOS and Linux.
//
// By default this links the patched JavaScriptCore built by scripts/jsc/build.sh
// from webkit/REVISION + webkit/patches (plus the C module shim in
// `shim/modules_source.c`) into the executable as a static library:
// lib/libJavaScriptCore.a holds JavaScriptCore, WTF and bmalloc. The loader then
// has no JavaScriptCore library to map and bind at launch, the linker drops what
// the browser never calls, and the patched entry points are ordinary linked
// symbols (`spi`). ICU and the C++ runtime stay shared: libicucore and libc++ on
// macOS, the distribution's libicu* and libstdc++ on Linux.
//
// A build directory that holds only lib/libJavaScriptCore.dylib (macOS, the
// layout before static linking; still useful through PYRO_JSC_DIR while iterating
// on a patch, since replacing a dylib needs no relink) is linked dynamically.
// Without a matching build macOS falls back to the system framework (public C
// API plus the ObjC module-loader shim in `shim/modules.m`) and warns;
// patched-only entry points are then missing and are reported unsupported at
// runtime (`spi`).
//
// PYRO_JSC=system forces the system framework; PYRO_JSC_DIR=<dir> uses the library in
// <dir>/lib; PYRO_JSC_CACHE overrides the artifact root build.sh writes to.
//
// On Linux only the patched source build is supported (there is no system
// JavaScriptCore with a module loader; WebKitGTK's libjavascriptcoregtk has none):
// scripts/jsc/build.sh writes lib/libJavaScriptCore.a under
// ${XDG_CACHE_HOME:-~/.cache}/pyro/jsc.
use std::path::{Path, PathBuf};
use std::{env, fs};

fn main() {
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap();
    if target_os != "macos" && target_os != "linux" {
        panic!("pyro-jsc-sys: only macOS and Linux are supported so far");
    }
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let inputs = manifest.join("webkit");
    for f in ["shim/modules.m", "shim/modules_source.c", "shim/fork_foundation.m", "webkit"] {
        println!("cargo:rerun-if-changed={f}");
    }
    for v in ["PYRO_JSC", "PYRO_JSC_DIR", "PYRO_JSC_CACHE", "HOME", "XDG_CACHE_HOME"] {
        println!("cargo:rerun-if-env-changed={v}");
    }
    println!("cargo:rustc-env=PYRO_JSC_INPUTS_HASH={:016x}", inputs_hash(&inputs));
    println!("cargo::rustc-check-cfg=cfg(pyro_jsc_source)");
    println!("cargo::rustc-check-cfg=cfg(pyro_jsc_static)");
    println!("cargo::rustc-check-cfg=cfg(pyro_jsc_adopt_global_proxy)");
    println!("cargo::rustc-check-cfg=cfg(pyro_jsc_custom_value)");
    println!("cargo::rustc-check-cfg=cfg(pyro_jsc_compile_strings)");
    println!("cargo::rustc-check-cfg=cfg(pyro_jsc_rejection_tracker)");
    println!("cargo::rustc-check-cfg=cfg(pyro_jsc_refuse_delete)");

    if target_os == "linux" {
        let Some(dir) = source_jsc(&inputs, &["libJavaScriptCore.a"], &linux_cache, false) else {
            panic!(
                "pyro-jsc-sys: no patched JavaScriptCore build for webkit/REVISION + patches in {}. \
                 Run scripts/jsc/build.sh or point PYRO_JSC_DIR at one (Linux has no system fallback).",
                linux_cache().map(|c| c.display().to_string()).unwrap_or_default()
            );
        };
        link_static(&dir.join("lib"));
        // What the archive needs from the system, after it on the link line:
        // ICU (the distribution's, which JSCOnly was configured against) and
        // the C++ runtime it was compiled with (clang's default on Linux).
        for lib in ["icui18n", "icuuc", "icudata", "stdc++"] {
            println!("cargo:rustc-link-lib=dylib={lib}");
        }
        // libpas uses 16-byte atomics, which clang leaves to libatomic on
        // x86-64 (the shared library linked it itself). Its archive comes with
        // the compiler, so the binary gains no run-time dependency.
        if env::var("CARGO_CFG_TARGET_ARCH").unwrap() == "x86_64" {
            println!("cargo:rustc-link-lib=static:-bundle=atomic");
        }
        source_shim(&dir.join("lib"));
        return;
    }

    // Foundation initialization is needed by forked DNS workers for either
    // JSC backend. Keep ObjC separate from the source build's plain C shim.
    println!("cargo:rustc-link-lib=framework=Foundation");
    cc::Build::new()
        .file("shim/fork_foundation.m")
        .flag("-fobjc-arc")
        .flag("-Wall")
        .flag("-Werror")
        .compile("pyro_foundation_fork");

    match source_jsc(&inputs, &["libJavaScriptCore.a", "libJavaScriptCore.dylib"], &macos_cache, true) {
        Some(dir) => {
            let lib = dir.join("lib");
            if lib.join("libJavaScriptCore.a").is_file() {
                link_static(&lib);
                // The system's ICU and C++ runtime; both are in the shared cache.
                println!("cargo:rustc-link-lib=dylib=icucore");
                println!("cargo:rustc-link-lib=dylib=c++");
            } else {
                println!("cargo:rerun-if-changed={}", lib.join("libJavaScriptCore.dylib").display());
                println!("cargo:rustc-link-search=native={}", lib.display());
                println!("cargo:rustc-link-lib=dylib=JavaScriptCore");
                println!("cargo:rustc-cfg=pyro_jsc_source");
                println!("cargo:jsc=source");
            }
            source_shim(&lib);
        }
        None => {
            println!("cargo:rustc-link-lib=framework=JavaScriptCore");
            println!("cargo:jsc=system");
            cc::Build::new().file("shim/modules.m").flag("-fobjc-arc").flag("-Wall").flag("-Werror").compile("pyro_jsc_shim");
        }
    }
}

/// Compiles the C module shim for the source-built JavaScriptCore in `lib`,
/// with patch 0037's module graph phases (JSLoadModuleGraph,
/// JSLinkAndEvaluateModule) only when that build has them, so a PYRO_JSC_DIR
/// of an older patch set still links (the shim then reports them missing).
fn source_shim(lib: &Path) {
    let mut build = cc::Build::new();
    build.file("shim/modules_source.c").flag("-Wall").flag("-Werror");
    if lib.parent().is_some_and(|dir| dir.join("inputs/patches/0037-module-graph-phases.patch").is_file()) {
        build.define("PYRO_JSC_MODULE_PHASES", None);
    }
    build.compile("pyro_jsc_shim");
}

/// Links `lib/libJavaScriptCore.a` (JavaScriptCore, WTF and bmalloc in one
/// archive, see scripts/jsc/build.sh) into every executable that uses this
/// crate. `-bundle`: the archive is named on the final link line instead of
/// being copied into this crate's rlib.
fn link_static(lib: &Path) {
    println!("cargo:rerun-if-changed={}", lib.join("libJavaScriptCore.a").display());
    println!("cargo:rustc-link-search=native={}", lib.display());
    println!("cargo:rustc-link-lib=static:-bundle=JavaScriptCore");
    println!("cargo:rustc-cfg=pyro_jsc_source");
    println!("cargo:rustc-cfg=pyro_jsc_static");
    println!("cargo:jsc=source");
    // Patch 0033 (JSGlobalContextAdoptGlobalProxy), linked by name: only when the
    // archive was built with it, so a PYRO_JSC_DIR of an older patch set still links.
    if lib.parent().is_some_and(|dir| dir.join("inputs/patches/0033-adopt-global-proxy.patch").is_file()) {
        println!("cargo:rustc-cfg=pyro_jsc_adopt_global_proxy");
    }
    // Patch 0034 (JSObjectSetCustomValue), the same way.
    if lib.parent().is_some_and(|dir| dir.join("inputs/patches/0034-custom-values.patch").is_file()) {
        println!("cargo:rustc-cfg=pyro_jsc_custom_value");
    }
    // Patch 0035 (JSGlobalContextSetCompileStringsCallbacks, Trusted Types for eval and
    // Function), the same way.
    if lib.parent().is_some_and(|dir| dir.join("inputs/patches/0035-compile-strings-hooks.patch").is_file()) {
        println!("cargo:rustc-cfg=pyro_jsc_compile_strings");
    }
    // Patch 0036 (JSGlobalContextSetPromiseRejectionTracker), the same way.
    if lib.parent().is_some_and(|dir| dir.join("inputs/patches/0036-promise-rejection-tracker.patch").is_file()) {
        println!("cargo:rustc-cfg=pyro_jsc_rejection_tracker");
    }
    // Patch 0039 (JSObjectRefuseDeleteProperty), the same way.
    if lib.parent().is_some_and(|dir| dir.join("inputs/patches/0039-refused-deletes.patch").is_file()) {
        println!("cargo:rustc-cfg=pyro_jsc_refuse_delete");
    }
}

fn macos_cache() -> Option<PathBuf> {
    Some(PathBuf::from(env::var_os("HOME")?).join("Library/Caches/pyro/jsc"))
}

fn linux_cache() -> Option<PathBuf> {
    let base = match env::var_os("XDG_CACHE_HOME") {
        Some(x) if !x.is_empty() => PathBuf::from(x),
        _ => PathBuf::from(env::var_os("HOME")?).join(".cache"),
    };
    Some(base.join("pyro/jsc"))
}

/// The build directory to link: `PYRO_JSC_DIR`, or the cached build that holds
/// one of `libnames` and was made from this checkout's inputs. A build with
/// the first name wins over one with a later name.
fn source_jsc(
    inputs: &Path,
    libnames: &[&str],
    default_cache: &dyn Fn() -> Option<PathBuf>,
    system_fallback: bool,
) -> Option<PathBuf> {
    if env::var("PYRO_JSC").as_deref() == Ok("system") {
        return None;
    }
    if let Ok(dir) = env::var("PYRO_JSC_DIR") {
        return Some(PathBuf::from(dir));
    }
    let cache = match env::var("PYRO_JSC_CACHE") {
        Ok(c) => PathBuf::from(c),
        Err(_) => default_cache()?,
    };
    println!("cargo:rerun-if-changed={}", cache.display());
    let dirs: Vec<PathBuf> = fs::read_dir(&cache).ok().into_iter().flatten().flatten().map(|e| e.path()).collect();
    let found = libnames.iter().find_map(|name| {
        dirs.iter().find(|dir| dir.join("lib").join(name).is_file() && same_inputs(inputs, &dir.join("inputs")))
    });
    if found.is_none() && system_fallback {
        println!(
            "cargo:warning=pyro-jsc-sys: no JavaScriptCore build for webkit/REVISION + patches in {}; \
             linking the system framework. Run scripts/jsc/build.sh (or set PYRO_JSC=system to silence this).",
            cache.display()
        );
    }
    found.cloned()
}

/// REVISION and the patches of `root`, by name.
fn input_files(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut out = vec![(PathBuf::from("REVISION"), fs::read(root.join("REVISION")).unwrap_or_default())];
    let mut patches: Vec<_> = fs::read_dir(root.join("patches")).into_iter().flatten().flatten().map(|e| e.path()).collect();
    patches.sort();
    for p in patches {
        out.push((PathBuf::from(p.file_name().unwrap()), fs::read(&p).unwrap_or_default()));
    }
    out
}

/// True when `built` holds exactly the REVISION and patches of `inputs`.
fn same_inputs(inputs: &Path, built: &Path) -> bool {
    input_files(inputs) == input_files(built)
}

/// FNV-1a over REVISION and the patches (names and contents): part of the
/// version of the bytecode cache, whose entries only the JavaScriptCore that
/// wrote them may read.
fn inputs_hash(inputs: &Path) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    let mut feed = |bytes: &[u8]| {
        for b in (bytes.len() as u64).to_le_bytes().iter().chain(bytes) {
            hash = (hash ^ u64::from(*b)).wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    for (name, contents) in input_files(inputs) {
        feed(name.to_string_lossy().as_bytes());
        feed(&contents);
    }
    hash
}
