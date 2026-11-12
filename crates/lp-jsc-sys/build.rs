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
// layout before static linking; still useful through LP_JSC_DIR while iterating
// on a patch, since replacing a dylib needs no relink) is linked dynamically.
// Without a matching build macOS falls back to the system framework (public C
// API plus the ObjC module-loader shim in `shim/modules.m`) and warns;
// patched-only entry points are then missing and are reported unsupported at
// runtime (`spi`).
//
// LP_JSC=system forces the system framework; LP_JSC_DIR=<dir> uses the library in
// <dir>/lib; LP_JSC_CACHE overrides the artifact root build.sh writes to.
//
// On Linux only the patched source build is supported (there is no system
// JavaScriptCore with a module loader; WebKitGTK's libjavascriptcoregtk has none):
// scripts/jsc/build.sh writes lib/libJavaScriptCore.a under
// ${XDG_CACHE_HOME:-~/.cache}/lightningpanda/jsc.
use std::path::{Path, PathBuf};
use std::{env, fs};

fn main() {
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap();
    if target_os != "macos" && target_os != "linux" {
        panic!("lp-jsc-sys: only macOS and Linux are supported so far");
    }
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let inputs = manifest.join("webkit");
    for f in ["shim/modules.m", "shim/modules_source.c", "shim/fork_foundation.m", "webkit"] {
        println!("cargo:rerun-if-changed={f}");
    }
    for v in ["LP_JSC", "LP_JSC_DIR", "LP_JSC_CACHE", "HOME", "XDG_CACHE_HOME"] {
        println!("cargo:rerun-if-env-changed={v}");
    }
    println!("cargo::rustc-check-cfg=cfg(lp_jsc_source)");
    println!("cargo::rustc-check-cfg=cfg(lp_jsc_static)");

    if target_os == "linux" {
        let Some(dir) = source_jsc(&inputs, &["libJavaScriptCore.a"], &linux_cache, false) else {
            panic!(
                "lp-jsc-sys: no patched JavaScriptCore build for webkit/REVISION + patches in {}. \
                 Run scripts/jsc/build.sh or point LP_JSC_DIR at one (Linux has no system fallback).",
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
        cc::Build::new().file("shim/modules_source.c").flag("-Wall").flag("-Werror").compile("lp_jsc_shim");
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
        .compile("lp_foundation_fork");

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
                println!("cargo:rustc-cfg=lp_jsc_source");
                println!("cargo:jsc=source");
            }
            cc::Build::new().file("shim/modules_source.c").flag("-Wall").flag("-Werror").compile("lp_jsc_shim");
        }
        None => {
            println!("cargo:rustc-link-lib=framework=JavaScriptCore");
            println!("cargo:jsc=system");
            cc::Build::new().file("shim/modules.m").flag("-fobjc-arc").flag("-Wall").flag("-Werror").compile("lp_jsc_shim");
        }
    }
}

/// Links `lib/libJavaScriptCore.a` (JavaScriptCore, WTF and bmalloc in one
/// archive, see scripts/jsc/build.sh) into every executable that uses this
/// crate. `-bundle`: the archive is named on the final link line instead of
/// being copied into this crate's rlib.
fn link_static(lib: &Path) {
    println!("cargo:rerun-if-changed={}", lib.join("libJavaScriptCore.a").display());
    println!("cargo:rustc-link-search=native={}", lib.display());
    println!("cargo:rustc-link-lib=static:-bundle=JavaScriptCore");
    println!("cargo:rustc-cfg=lp_jsc_source");
    println!("cargo:rustc-cfg=lp_jsc_static");
    println!("cargo:jsc=source");
}

fn macos_cache() -> Option<PathBuf> {
    Some(PathBuf::from(env::var_os("HOME")?).join("Library/Caches/lightningpanda/jsc"))
}

fn linux_cache() -> Option<PathBuf> {
    let base = match env::var_os("XDG_CACHE_HOME") {
        Some(x) if !x.is_empty() => PathBuf::from(x),
        _ => PathBuf::from(env::var_os("HOME")?).join(".cache"),
    };
    Some(base.join("lightningpanda/jsc"))
}

/// The build directory to link: `LP_JSC_DIR`, or the cached build that holds
/// one of `libnames` and was made from this checkout's inputs. A build with
/// the first name wins over one with a later name.
fn source_jsc(
    inputs: &Path,
    libnames: &[&str],
    default_cache: &dyn Fn() -> Option<PathBuf>,
    system_fallback: bool,
) -> Option<PathBuf> {
    if env::var("LP_JSC").as_deref() == Ok("system") {
        return None;
    }
    if let Ok(dir) = env::var("LP_JSC_DIR") {
        return Some(PathBuf::from(dir));
    }
    let cache = match env::var("LP_JSC_CACHE") {
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
            "cargo:warning=lp-jsc-sys: no JavaScriptCore build for webkit/REVISION + patches in {}; \
             linking the system framework. Run scripts/jsc/build.sh (or set LP_JSC=system to silence this).",
            cache.display()
        );
    }
    found.cloned()
}

/// True when `built` holds exactly the REVISION and patches of `inputs`.
fn same_inputs(inputs: &Path, built: &Path) -> bool {
    let files = |root: &Path| -> Vec<(PathBuf, Vec<u8>)> {
        let mut out = vec![(PathBuf::from("REVISION"), fs::read(root.join("REVISION")).unwrap_or_default())];
        let mut patches: Vec<_> = fs::read_dir(root.join("patches")).into_iter().flatten().flatten().map(|e| e.path()).collect();
        patches.sort();
        for p in patches {
            out.push((PathBuf::from(p.file_name().unwrap()), fs::read(&p).unwrap_or_default()));
        }
        out
    };
    files(inputs) == files(built)
}
