//! The bytecode cache (`pyro_js::bytecode_cache`, JavaScriptCore patch 0031):
//! a source evaluated once is written to a file, a later load in another
//! engine takes its text, record and code from that file, and a file that is
//! not exactly what was written is refused and removed.

use super::precompiled_modules::{load, EXPECTED, GRAPH};
use pyro_js::bytecode_cache::{BytecodeCache, StoreStats, WriteMode};
use pyro_js::*;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// A directory of this test alone, removed when dropped.
struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Dir {
        let dir = std::env::temp_dir().join(format!("pyro-bytecode-cache-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        Dir(dir)
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A cache of `dir` that takes every source and writes before going on.
fn open(dir: &Path) -> Arc<BytecodeCache> {
    let cache = BytecodeCache::open(dir).expect("cache directory");
    cache.set_min_source_len(0);
    cache.set_write_mode(WriteMode::Inline);
    cache
}

/// Runs `f` with a new engine that uses `cache`, on a thread of its own (an
/// engine per thread): nothing of an earlier engine's compiled code is there.
fn with_engine<R: Send + 'static>(cache: &Arc<BytecodeCache>, f: impl FnOnce(&Engine) -> R + Send + 'static) -> R {
    let cache = cache.clone();
    std::thread::spawn(move || {
        let e = Engine::new().expect("engine");
        e.set_bytecode_cache(Some(cache));
        f(&e)
    })
    .join()
    .expect("engine thread")
}

/// A classic script whose functions are compiled at their first call.
const SCRIPT: &str = "var calls = 0;\n\
    function fib(n) { calls++; return n < 2 ? n : fib(n - 1) + fib(n - 2); }\n\
    class Box { #v; constructor(v) { this.#v = v; } get twice() { return this.#v * 2; } }\n\
    var later = (a, b = 2) => { try { null.x } catch (e) { return `${a * b}:${e.line}:${e.sourceURL}` } };\n\
    function unused() { return 'never called ' + [1, 2, 3].map((x) => x * 2).join(); }\n\
    String(fib(10)) + '|' + new Box(21).twice;\n";

fn run_script(e: &Engine) -> String {
    let realm = e.create_realm(RealmOptions::default()).unwrap();
    let s = realm.scope().unwrap();
    let first = s.eval(SCRIPT, Some("http://site.test/app.js"), 1).unwrap().to_string().unwrap();
    // Functions first called after the script ran.
    let later = s.eval("later(4) + '|' + unused() + '|' + calls", None, 1).unwrap().to_string().unwrap();
    format!("{first} {later}")
}

/// What [`run_script`] evaluates: its two scripts, and the helpers script
/// the engine itself runs in every realm, which is a source like any other.
const SOURCES: u64 = 3;

const SCRIPT_RESULT: &str = "55|42 8:4:http://site.test/app.js|never called 2,4,6|177";

fn entries(cache: &BytecodeCache) -> Vec<PathBuf> {
    let dir = cache.version_dir().expect("the cache was used").to_path_buf();
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().path()).collect();
    files.sort();
    files
}

fn supported() -> bool {
    if !BytecodeCache::supported() {
        eprintln!("bytecode cache unavailable; skipping");
    }
    BytecodeCache::supported()
}

#[test]
fn a_script_is_written_at_its_first_run_and_decoded_at_the_next() {
    if !supported() {
        return;
    }
    let dir = Dir::new("script");
    let cache = open(&dir.0);
    let (result, jsc) = with_engine(&cache, |e| (run_script(e), e.bytecode_cache_stats()));
    assert_eq!(result, SCRIPT_RESULT);
    // A miss: evaluated from its source, then written. The second, short
    // script too (no minimum here).
    assert_eq!(cache.stats(), StoreStats { misses: SOURCES, written: SOURCES, ..Default::default() });
    assert_eq!(jsc, BytecodeCacheStats::default());
    assert_eq!(entries(&cache).len() as u64, SOURCES);
    for entry in entries(&cache) {
        assert_eq!(std::fs::metadata(&entry).unwrap().permissions().mode() & 0o777, 0o600, "{entry:?}");
    }
    assert_eq!(std::fs::metadata(&dir.0).unwrap().permissions().mode() & 0o777, 0o700);

    // Another process' view: a cache opened anew, an engine with no code.
    let cache = open(&dir.0);
    let (result, jsc) = with_engine(&cache, |e| (run_script(e), e.bytecode_cache_stats()));
    assert_eq!(result, SCRIPT_RESULT);
    assert_eq!(cache.stats(), StoreStats { hits: SOURCES, ..Default::default() });
    assert_eq!(jsc.code_accepted, SOURCES as usize, "{jsc:?}");
    assert_eq!(jsc.code_rejected, 0, "{jsc:?}");
}

#[test]
fn another_text_kind_or_host_is_another_entry() {
    if !supported() {
        return;
    }
    let dir = Dir::new("miss");
    let cache = open(&dir.0);
    let run = |source: &'static str, url: &'static str| {
        with_engine(&cache, move |e| {
            let realm = e.create_realm(RealmOptions::default()).unwrap();
            let s = realm.scope().unwrap();
            s.eval(source, Some(url), 1).unwrap().to_string().unwrap()
        })
    };
    // Hits, misses and writes of the script alone (not of the helpers script
    // every realm runs: a miss in the first engine, a hit in each later one).
    let engines = std::cell::Cell::new(0);
    let counts = |cache: &BytecodeCache| {
        let s = cache.stats();
        (s.hits + 1 - engines.get(), s.misses - 1, s.written - 1)
    };
    let run = |source: &'static str, url: &'static str| {
        engines.set(engines.get() + 1);
        run(source, url)
    };
    assert_eq!(run("var a = 1; a + 1", "http://one.test/a.js"), "2");
    assert_eq!(counts(&cache), (0, 1, 1));
    // The same text from the same host, whatever the path and the port: a hit.
    assert_eq!(run("var a = 1; a + 1", "http://one.test:8080/b.js"), "2");
    assert_eq!(counts(&cache), (1, 1, 1));
    // One character more, and another host: entries of their own.
    assert_eq!(run("var a = 1; a + 2", "http://one.test/a.js"), "3");
    assert_eq!(run("var a = 1; a + 1", "http://two.test/a.js"), "2");
    assert_eq!(counts(&cache), (1, 3, 3));
    // The same text as a module is not the script's entry.
    let module = with_engine(&cache, |e| {
        let realm = e.create_realm(RealmOptions { origin: Some("http://one.test".into()), ..Default::default() }).unwrap();
        let s = realm.scope().unwrap();
        s.eval_module("var a = 1; globalThis.m = a + 1", "http://one.test/a.js").unwrap();
        s.eval("String(globalThis.m)", None, 1).unwrap().to_string().unwrap()
    });
    assert_eq!(module, "2");
    engines.set(engines.get() + 1);
    assert_eq!(counts(&cache), (1, 5, 5), "the module and the script that reads its result");
}

#[test]
fn a_text_that_is_not_ascii_is_cached_too() {
    if !supported() {
        return;
    }
    // Latin-1 only (JavaScriptCore keeps such a text in one byte per
    // character when it compiles it), and beyond: the entry holds UTF-16.
    for (name, source, expected) in [
        ("latin1", "function caf\u{e9}(n) { return 'd\u{e9}j\u{e0} ' + n; }\ncaf\u{e9}(2) + '|' + 'd\u{e9}j\u{e0}'.length", "d\u{e9}j\u{e0} 2|4"),
        ("utf16", "function f(n) { return '\u{2192}\u{1f600} ' + n; }\nvar \u{3b1} = f(3);\n\u{3b1} + '|' + \u{3b1}.length", "\u{2192}\u{1f600} 3|5"),
    ] {
        let dir = Dir::new(name);
        for round in 0..2 {
            let cache = open(&dir.0);
            let (result, jsc) = with_engine(&cache, move |e| {
                let realm = e.create_realm(RealmOptions::default()).unwrap();
                let s = realm.scope().unwrap();
                (s.eval(source, Some("http://site.test/a.js"), 1).unwrap().to_string().unwrap(), e.bytecode_cache_stats())
            });
            assert_eq!(result, expected, "{name} {round}");
            // The script and the helpers script.
            let expected = if round == 0 {
                StoreStats { misses: 2, written: 2, ..Default::default() }
            } else {
                StoreStats { hits: 2, ..Default::default() }
            };
            assert_eq!(cache.stats(), expected, "{name} {round}");
            assert_eq!((jsc.code_accepted, jsc.code_rejected), (2 * round, 0), "{name} {round} {jsc:?}");
        }
    }
}

#[test]
fn an_entry_of_another_version_is_never_read() {
    if !supported() {
        return;
    }
    let dir = Dir::new("version");
    let with_version = |version: u64| {
        let cache = BytecodeCache::open_with_version(&dir.0, version).unwrap();
        cache.set_min_source_len(0);
        cache.set_write_mode(WriteMode::Inline);
        cache
    };
    let old = with_version(1);
    assert_eq!(with_engine(&old, run_script), SCRIPT_RESULT);
    assert_eq!(old.stats().written, SOURCES);

    // Another build: its entries live elsewhere, and it compiles and writes.
    let new = with_version(2);
    assert_eq!(with_engine(&new, run_script), SCRIPT_RESULT);
    assert_eq!(new.stats(), StoreStats { misses: SOURCES, written: SOURCES, ..Default::default() });
    assert_ne!(old.version_dir(), new.version_dir());

    // An entry of the other build under this build's name (a copied
    // directory): refused by the version it carries, removed, written anew.
    for (from, to) in entries(&old).iter().zip(entries(&new)) {
        std::fs::copy(from, &to).unwrap();
        std::fs::set_permissions(&to, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let new = with_version(2);
    let (result, jsc) = with_engine(&new, |e| (run_script(e), e.bytecode_cache_stats()));
    assert_eq!(result, SCRIPT_RESULT);
    assert_eq!(new.stats(), StoreStats { misses: 0, rejected: SOURCES, written: SOURCES, ..Default::default() });
    assert_eq!(jsc, BytecodeCacheStats::default(), "nothing of a refused entry reaches JavaScriptCore");
}

/// Damages the entries of `dir` with `damage`, then checks that a load
/// refuses every one of them, gives JavaScriptCore nothing of them, runs
/// the script from its source and writes the entries anew.
fn damaged_entries_are_refused(name: &str, damage: impl Fn(&Path)) {
    let dir = Dir::new(name);
    let cache = open(&dir.0);
    assert_eq!(with_engine(&cache, run_script), SCRIPT_RESULT);
    for entry in entries(&cache) {
        damage(&entry);
    }
    let cache = open(&dir.0);
    let (result, jsc) = with_engine(&cache, |e| (run_script(e), e.bytecode_cache_stats()));
    assert_eq!(result, SCRIPT_RESULT, "{name}");
    assert_eq!(cache.stats(), StoreStats { rejected: SOURCES, written: SOURCES, ..Default::default() }, "{name}");
    assert_eq!(jsc, BytecodeCacheStats::default(), "{name}");
    // What was written anew is read.
    let cache = open(&dir.0);
    let (result, jsc) = with_engine(&cache, |e| (run_script(e), e.bytecode_cache_stats()));
    assert_eq!(result, SCRIPT_RESULT, "{name}");
    assert_eq!(cache.stats(), StoreStats { hits: SOURCES, ..Default::default() }, "{name}");
    assert_eq!(jsc.code_accepted, SOURCES as usize, "{name}");
}

#[test]
fn a_corrupted_entry_is_refused_and_removed() {
    if !supported() {
        return;
    }
    // One bit, in the bytecode (the head of the file), in the middle, and in
    // the description at its end.
    for (name, at) in [("flip-head", 0.0), ("flip-early", 0.1), ("flip-middle", 0.5), ("flip-late", 0.97), ("flip-last", 1.0)] {
        damaged_entries_are_refused(name, |entry| {
            let mut bytes = std::fs::read(entry).unwrap();
            let index = ((bytes.len() - 1) as f64 * at) as usize;
            bytes[index] ^= 0x10;
            std::fs::write(entry, bytes).unwrap();
        });
    }
}

#[test]
fn a_truncated_or_extended_entry_is_refused_and_removed() {
    if !supported() {
        return;
    }
    for (name, keep) in [("cut-empty", 0.0), ("cut-short", 0.01), ("cut-half", 0.5), ("cut-one-byte", 1.0)] {
        damaged_entries_are_refused(name, |entry| {
            let bytes = std::fs::read(entry).unwrap();
            let len = ((bytes.len() - 1) as f64 * keep) as usize;
            std::fs::write(entry, &bytes[..len]).unwrap();
        });
    }
    damaged_entries_are_refused("extended", |entry| {
        let mut bytes = std::fs::read(entry).unwrap();
        bytes.extend_from_slice(&[0u8; 4096]);
        std::fs::write(entry, bytes).unwrap();
    });
    // Two whole entries, one after the other: the end is a valid description
    // of a shorter file.
    damaged_entries_are_refused("doubled", |entry| {
        let bytes = std::fs::read(entry).unwrap();
        std::fs::write(entry, [bytes.clone(), bytes].concat()).unwrap();
    });
}

#[test]
fn an_entry_others_can_write_or_a_link_is_refused() {
    if !supported() {
        return;
    }
    damaged_entries_are_refused("mode", |entry| {
        std::fs::set_permissions(entry, std::fs::Permissions::from_mode(0o644)).unwrap();
    });
    damaged_entries_are_refused("link", |entry| {
        let moved = entry.with_extension("moved");
        std::fs::rename(entry, &moved).unwrap();
        std::os::unix::fs::symlink(&moved, entry).unwrap();
    });
    // Every entry under the name of another source: the first call moves
    // them all one name on.
    let swapped = std::cell::Cell::new(false);
    damaged_entries_are_refused("swapped", |entry| {
        if swapped.replace(true) {
            return;
        }
        let mut names: Vec<PathBuf> = std::fs::read_dir(entry.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_none() && !p.file_name().unwrap().to_str().unwrap().starts_with('.'))
            .collect();
        names.sort();
        assert_eq!(names.len(), SOURCES as usize);
        let bytes: Vec<Vec<u8>> = names.iter().map(|p| std::fs::read(p).unwrap()).collect();
        for (i, name) in names.iter().enumerate() {
            std::fs::write(name, &bytes[(i + 1) % bytes.len()]).unwrap();
        }
    });
    // A directory open to others is not used at all.
    let dir = Dir::new("open-dir");
    std::fs::create_dir_all(&dir.0).unwrap();
    std::fs::set_permissions(&dir.0, std::fs::Permissions::from_mode(0o755)).unwrap();
    let error = BytecodeCache::open(&dir.0).err().expect("refused");
    assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied, "{error}");
}

#[test]
fn modules_load_from_the_cache_as_from_their_source() {
    if !supported() {
        return;
    }
    let dir = Dir::new("modules");
    let cache = open(&dir.0);
    // The modules, the helpers script of the realm and the script that reads
    // the result.
    let sources = GRAPH.len() as u64 + 2;
    let first = with_engine(&cache, |e| load(e, "http://site.test", GRAPH));
    assert_eq!(first, EXPECTED);
    assert_eq!(cache.stats(), StoreStats { misses: sources, written: sources, ..Default::default() });

    let cache = open(&dir.0);
    let (other, second, before, after, compiled) = with_engine(&cache, |e| {
        // The same graph from another origin is other text (`import.meta.url`):
        // compiled from source and written. That also compiles the engine's
        // own functions (builtins) these modules call, which no entry holds.
        let other = load(e, "http://warm.test", GRAPH);
        let before = e.bytecode_cache_stats();
        let compiled = e.module_precompile_stats().function_compiles;
        let out = load(e, "http://site.test", GRAPH);
        (other, out, before, e.bytecode_cache_stats(), e.module_precompile_stats().function_compiles - compiled)
    });
    assert_eq!(other, EXPECTED.replace("site.test", "warm.test"));
    assert_eq!(second, EXPECTED);
    // The other origin's modules are new. The helpers script and the script
    // that reads the result (it has no host) are found by both realms, by
    // the second one among the entries the engine still holds mapped.
    let new = GRAPH.len() as u64;
    assert_eq!(
        cache.stats(),
        StoreStats { hits: sources + 2, mapped_hits: 2, misses: new, written: new, ..Default::default() }
    );
    // Every module's record and code came from its entry, and no function of
    // theirs was compiled from source: constructors, generators, async
    // functions, accessors and nested functions are all in the entry.
    assert_eq!(after.record_accepted - before.record_accepted, GRAPH.len(), "{after:?}");
    assert_eq!(after.code_accepted - before.code_accepted, GRAPH.len(), "{before:?} {after:?}");
    assert_eq!((after.record_rejected, after.code_rejected), (0, 0), "{after:?}");
    assert_eq!(compiled, 0);
}

#[test]
fn precompiled_modules_come_from_the_cache() {
    if !supported() || !Engine::has_module_precompile() {
        return;
    }
    let dir = Dir::new("precompile");
    for round in 0..2 {
        let cache = open(&dir.0);
        let (out, jsc, modules) = with_engine(&cache, |e| {
            let kept = GRAPH
                .iter()
                .filter(|(path, source)| e.precompile_module(source, &format!("http://site.test{path}"), false))
                .count();
            assert_eq!(kept, GRAPH.len());
            let before = e.module_precompile_stats();
            let out = load(e, "http://site.test", GRAPH);
            let after = e.module_precompile_stats();
            assert_eq!(after.record_hits - before.record_hits, GRAPH.len(), "{after:?}");
            assert_eq!(after.code_hits - before.code_hits, GRAPH.len(), "{after:?}");
            (out, e.bytecode_cache_stats(), after.modules)
        });
        assert_eq!(out, EXPECTED, "round {round}");
        assert_eq!(modules, GRAPH.len());
        // The first round writes the entries and precompiles from them; the
        // second finds them.
        let stats = cache.stats();
        assert_eq!(stats.written, if round == 0 { GRAPH.len() as u64 + 2 } else { 0 }, "{stats:?}");
        assert_eq!(jsc.record_accepted, GRAPH.len(), "round {round}: {jsc:?}");
        // The second round's realm also takes its helpers script and the
        // script that reads the result from the cache.
        assert_eq!(jsc.code_accepted, GRAPH.len() + if round == 0 { 0 } else { 2 }, "round {round}: {jsc:?}");
    }
}

#[test]
fn the_background_writer_writes_what_was_evaluated() {
    if !supported() {
        return;
    }
    let dir = Dir::new("background");
    let cache = BytecodeCache::open(&dir.0).unwrap();
    cache.set_min_source_len(0);
    assert_eq!(cache.write_mode(), WriteMode::Background);
    assert_eq!(with_engine(&cache, run_script), SCRIPT_RESULT);
    assert!(cache.flush(Duration::from_secs(120)));
    assert_eq!(cache.stats(), StoreStats { misses: SOURCES, written: SOURCES, ..Default::default() });

    let cache = open(&dir.0);
    let (result, jsc) = with_engine(&cache, |e| (run_script(e), e.bytecode_cache_stats()));
    assert_eq!(result, SCRIPT_RESULT);
    assert_eq!(jsc.code_accepted, SOURCES as usize, "{jsc:?}");

    // Nothing is written with writing off, and short sources are left alone
    // (the helpers script is not short).
    let dir = Dir::new("off");
    let cache = BytecodeCache::open(&dir.0).unwrap();
    cache.set_write_mode(WriteMode::Off);
    assert_eq!(with_engine(&cache, run_script), SCRIPT_RESULT);
    assert_eq!(cache.stats(), StoreStats { misses: 1, ..Default::default() });
    cache.set_min_source_len(0);
    assert_eq!(with_engine(&cache, run_script), SCRIPT_RESULT);
    assert!(cache.flush(Duration::from_secs(1)));
    assert_eq!(cache.stats(), StoreStats { misses: 1 + SOURCES, ..Default::default() });
}
