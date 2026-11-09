// Links JavaScriptCore on macOS and Linux.
//
// By default this links the patched JavaScriptCore dylib built by
// scripts/jsc/build.sh from webkit/REVISION + webkit/patches (plus the C module
// shim in `shim/modules_source.c`). Without a matching build it falls back to the
// system framework (public C API plus the ObjC module-loader shim in
// `shim/modules.m`) and warns; patched-only entry points are then missing and
// are reported unsupported at runtime (`spi`).
//
// LP_JSC=system forces the system framework; LP_JSC_DIR=<dir> uses the dylib in
// <dir>/lib; LP_JSC_CACHE overrides the artifact root build.sh writes to.
//
// On Linux only the patched source build is supported (there is no system
// JavaScriptCore with a module loader; WebKitGTK's libjavascriptcoregtk has none):
// scripts/jsc/build.sh writes lib/libJavaScriptCore.so under
// ${XDG_CACHE_HOME:-~/.cache}/lightningpanda/jsc. Its SONAME is the absolute path,
// so linked binaries load it without an rpath, as the dylib's install name does.
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

    if target_os == "linux" {
        let Some(dir) = source_jsc(&inputs, "libJavaScriptCore.so", &linux_cache) else {
            panic!(
                "lp-jsc-sys: no patched JavaScriptCore build for webkit/REVISION + patches in {}. \
                 Run scripts/jsc/build.sh or point LP_JSC_DIR at one (Linux has no system fallback).",
                linux_cache().map(|c| c.display().to_string()).unwrap_or_default()
            );
        };
        let lib = dir.join("lib");
        println!("cargo:rerun-if-changed={}", lib.join("libJavaScriptCore.so").display());
        println!("cargo:rustc-link-search=native={}", lib.display());
        println!("cargo:rustc-link-lib=dylib=JavaScriptCore");
        println!("cargo:rustc-cfg=lp_jsc_source");
        println!("cargo:jsc=source");
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

    match source_jsc(&inputs, "libJavaScriptCore.dylib", &macos_cache) {
        Some(dir) => {
            let lib = dir.join("lib");
            println!("cargo:rerun-if-changed={}", lib.join("libJavaScriptCore.dylib").display());
            println!("cargo:rustc-link-search=native={}", lib.display());
            println!("cargo:rustc-link-lib=dylib=JavaScriptCore");
            println!("cargo:rustc-cfg=lp_jsc_source");
            println!("cargo:jsc=source");
            cc::Build::new().file("shim/modules_source.c").flag("-Wall").flag("-Werror").compile("lp_jsc_shim");
        }
        None => {
            println!("cargo:rustc-link-lib=framework=JavaScriptCore");
            println!("cargo:jsc=system");
            cc::Build::new().file("shim/modules.m").flag("-fobjc-arc").flag("-Wall").flag("-Werror").compile("lp_jsc_shim");
        }
    }
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

fn source_jsc(inputs: &Path, libname: &str, default_cache: &dyn Fn() -> Option<PathBuf>) -> Option<PathBuf> {
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
    let found = fs::read_dir(&cache).ok().into_iter().flatten().flatten().map(|e| e.path()).find(|dir| {
        dir.join("lib").join(libname).is_file() && same_inputs(inputs, &dir.join("inputs"))
    });
    if found.is_none() && libname.ends_with(".dylib") {
        println!(
            "cargo:warning=lp-jsc-sys: no JavaScriptCore build for webkit/REVISION + patches in {}; \
             linking the system framework. Run scripts/jsc/build.sh (or set LP_JSC=system to silence this).",
            cache.display()
        );
    }
    found
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
