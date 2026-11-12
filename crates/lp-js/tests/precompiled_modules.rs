//! Modules parsed and compiled ahead of their first load
//! (`Engine::precompile_module`, JavaScriptCore patch 0027): a load of the
//! same text takes the module's record and code from the engine, and behaves
//! as a load that parses and compiles.

use lp_js::*;
use std::rc::Rc;

fn eval_str(realm: &Realm, src: &str) -> String {
    let s = realm.scope().unwrap();
    match s.eval(src, Some("test.js"), 1) {
        Ok(v) => v.to_string().unwrap(),
        Err(e) => format!("THROW {}", e.details(&s).text()),
    }
}

/// A module graph that uses what a record and its code carry: named, default,
/// star and namespace exports, a renamed live binding, classes with private
/// fields, generators, async functions, accessors, a dynamic import, a module
/// with top-level await, `import.meta`, a `sourceURL` comment, and an error's
/// position.
pub(super) const GRAPH: &[(&str, &str)] = &[
    (
        "/app/main.js",
        "import def, { count, inc, Shape, gen, later, pair, fail } from './lib.js';\n\
         import * as all from './all.js';\n\
         import { ns, renamed } from './all.js';\n\
         inc(); inc();\n\
         const shape = new Shape(3);\n\
         const made = [...gen(3)].join('');\n\
         const awaited = await later(5);\n\
         const lazy = await import('./lazy.js');\n\
         let thrown;\n\
         try { fail() } catch (e) { thrown = e.message + '@' + e.sourceURL + ':' + e.line + ':' + e.column }\n\
         globalThis.out = [def, count, shape.area, shape.describe(), Shape.make(2).area, made, awaited,\n\
             lazy.default(2), lazy.where, import.meta.url, Object.keys(all).sort().join(','),\n\
             ns.count === count, renamed, pair.sum, typeof all.default, thrown].join('|');\n",
    ),
    (
        "/app/lib.js",
        "export let count = 0;\n\
         export function inc() { return ++count; }\n\
         export default 'lib';\n\
         export class Shape {\n\
           #side;\n\
           label = 'shape';\n\
           constructor(side) { this.#side = side; }\n\
           get area() { return this.#side ** 2; }\n\
           describe() { return `${this.constructor.name}:${this.label}:${this.#side}`; }\n\
           static make(side) { return new Shape(side); }\n\
         }\n\
         export function* gen(n) { for (let i = 0; i < n; i++) yield i; }\n\
         export const later = async (v) => { await null; return v * 2; };\n\
         export const pair = { a: 1, b: 2, get sum() { return this.a + this.b; } };\n\
         export function fail() { const inner = () => { throw new Error('boom'); }; inner(); }\n",
    ),
    (
        "/app/all.js",
        "export * from './lib.js';\n\
         export * as ns from './lib.js';\n\
         export { count as renamed } from './lib.js';\n",
    ),
    (
        "/app/lazy.js",
        "const base = await Promise.resolve(40);\n\
         export default function (n) { return base + n; }\n\
         export const where = (() => { try { null.x } catch (e) { return e.sourceURL } })();\n\
         //# sourceURL=named-lazy.js\n",
    ),
];

pub(super) struct Files {
    pub(super) origin: &'static str,
    pub(super) files: &'static [(&'static str, &'static str)],
}

impl ModuleFetcher for Files {
    fn fetch(&self, realm: &Realm, url: String, request: ModuleRequest) {
        let s = realm.scope().unwrap();
        let path = url.strip_prefix(self.origin).unwrap_or(&url);
        match self.files.iter().find(|(p, _)| *p == path) {
            Some((_, source)) => request.resolve(&s, source).unwrap(),
            None => request.reject(&s, &JsError::Type(format!("404 {url}").into())).unwrap(),
        }
    }
}

/// Loads the graph from `origin` in a new realm; what its entry module left
/// in `globalThis.out`.
pub(super) fn load(engine: &Engine, origin: &'static str, files: &'static [(&'static str, &'static str)]) -> String {
    let realm = engine.create_realm(RealmOptions { origin: Some(origin.into()), ..Default::default() }).unwrap();
    realm.set_module_fetcher(Some(Rc::new(Files { origin, files })));
    let s = realm.scope().unwrap();
    let (path, source) = files[0];
    s.eval_module(source, &format!("{origin}{path}")).unwrap();
    drop(s);
    eval_str(&realm, "String(globalThis.out)")
}

fn precompile(engine: &Engine, origin: &str, files: &[(&str, &str)], functions: bool) -> usize {
    files
        .iter()
        .filter(|(path, source)| engine.precompile_module(source, &format!("{origin}{path}"), functions))
        .count()
}

// The error's line counts the `import.meta.url` line the loader puts first.
pub(super) const EXPECTED: &str = "lib|2|9|Shape:shape:3|4|012|10|42|http://site.test/app/lazy.js|http://site.test/app/main.js|\
                        Shape,count,fail,gen,inc,later,ns,pair,renamed|true|2|3|undefined|\
                        boom@http://site.test/app/lib.js:16:63";

/// What the counters gained since `before`.
fn gained(engine: &Engine, before: ModulePrecompileStats) -> ModulePrecompileStats {
    let now = engine.module_precompile_stats();
    ModulePrecompileStats {
        modules: now.modules,
        functions: now.functions,
        characters: now.characters,
        record_hits: now.record_hits - before.record_hits,
        record_misses: now.record_misses - before.record_misses,
        code_hits: now.code_hits - before.code_hits,
        function_compiles: now.function_compiles - before.function_compiles,
    }
}

#[test]
fn a_precompiled_module_loads_as_a_compiled_one_and_compiles_nothing() {
    if !Engine::has_module_precompile() {
        eprintln!("module precompile unavailable; skipping");
        return;
    }
    let e = Engine::new().expect("engine");
    // The reference, from another origin: its text differs (`import.meta.url`
    // is part of it), so nothing of it serves the loads below. It also
    // compiles the engine's own functions (builtins) these modules call.
    let reference = load(&e, "http://warm.test", GRAPH);
    assert_eq!(reference, EXPECTED.replace("site.test", "warm.test"));

    assert_eq!(precompile(&e, "http://site.test", GRAPH, true), GRAPH.len());
    let kept = e.module_precompile_stats();
    assert_eq!(kept.modules, GRAPH.len());
    // inc, Shape's constructor, initializer, getter and two methods, gen,
    // later, pair's getter, fail and its arrow, lazy's two functions: 13 at
    // least (generators and async functions have a body function each).
    assert!(kept.functions >= 13, "{kept:?}");
    // Precompiling again keeps what is there.
    assert_eq!(precompile(&e, "http://site.test", GRAPH, true), GRAPH.len());
    assert_eq!(e.module_precompile_stats(), kept);

    // Twice: a second realm (a reload) is served the same way and has its
    // own module instances.
    for _ in 0..2 {
        let before = e.module_precompile_stats();
        assert_eq!(load(&e, "http://site.test", GRAPH), EXPECTED);
        let stats = gained(&e, before);
        assert_eq!(
            (stats.record_hits, stats.record_misses, stats.code_hits, stats.function_compiles),
            (GRAPH.len(), 0, GRAPH.len(), 0),
            "{stats:?}"
        );
    }
}

#[test]
fn without_its_functions_a_precompiled_module_compiles_them_when_called() {
    if !Engine::has_module_precompile() {
        return;
    }
    let e = Engine::new().expect("engine");
    load(&e, "http://warm.test", GRAPH);
    assert_eq!(precompile(&e, "http://site.test", GRAPH, false), GRAPH.len());
    assert_eq!(e.module_precompile_stats().functions, 0);
    let before = e.module_precompile_stats();
    assert_eq!(load(&e, "http://site.test", GRAPH), EXPECTED);
    let stats = gained(&e, before);
    assert_eq!((stats.record_hits, stats.record_misses, stats.code_hits), (GRAPH.len(), 0, GRAPH.len()), "{stats:?}");
    assert!(stats.function_compiles >= 10, "{stats:?}");
}

#[test]
fn precompiled_modules_survive_collections() {
    if !Engine::has_module_precompile() {
        return;
    }
    let e = Engine::new().expect("engine");
    load(&e, "http://warm.test", GRAPH);
    assert_eq!(precompile(&e, "http://site.test", GRAPH, true), GRAPH.len());
    for _ in 0..3 {
        assert!(e.collect_garbage_now());
    }
    e.settle();
    let before = e.module_precompile_stats();
    assert_eq!(load(&e, "http://site.test", GRAPH), EXPECTED);
    e.collect_garbage_now();
    assert_eq!(load(&e, "http://site.test", GRAPH), EXPECTED);
    let stats = gained(&e, before);
    assert_eq!(
        (stats.record_hits, stats.record_misses, stats.code_hits, stats.function_compiles),
        (2 * GRAPH.len(), 0, 2 * GRAPH.len(), 0),
        "{stats:?}"
    );
}

#[test]
fn another_text_at_a_precompiled_url_is_parsed() {
    if !Engine::has_module_precompile() {
        return;
    }
    const OLD: &[(&str, &str)] = &[("/m.js", "globalThis.out = 'old';")];
    const NEW: &[(&str, &str)] = &[("/m.js", "globalThis.out = 'new';")];
    let e = Engine::new().expect("engine");
    assert_eq!(precompile(&e, "http://site.test", OLD, true), 1);
    let before = e.module_precompile_stats();
    assert_eq!(load(&e, "http://site.test", NEW), "new");
    // The same text from another origin is another module (`import.meta.url`).
    assert_eq!(load(&e, "http://other.test", OLD), "old");
    let stats = gained(&e, before);
    assert_eq!((stats.record_hits, stats.record_misses, stats.code_hits), (0, 2, 0), "{stats:?}");
    assert_eq!(load(&e, "http://site.test", OLD), "old");
    assert_eq!(gained(&e, before).record_hits, 1);
}

#[test]
fn a_source_that_is_no_module_is_not_kept() {
    if !Engine::has_module_precompile() {
        return;
    }
    const BROKEN: &[(&str, &str)] = &[
        ("/main.js", "globalThis.out = await import('./broken.js').then(() => 'loaded', (e) => e.name);"),
        ("/broken.js", "export const = 1;"),
    ];
    let e = Engine::new().expect("engine");
    assert!(!e.precompile_module(BROKEN[1].1, "http://site.test/broken.js", true));
    // Sloppy-mode script text is no module either.
    assert!(!e.precompile_module("with (Math) { var x = PI }", "http://site.test/classic.js", true));
    assert!(!e.precompile_module("export {}", "not a url", true));
    assert_eq!(e.module_precompile_stats().modules, 0);
    assert!(e.precompile_module(BROKEN[0].1, "http://site.test/main.js", true));
    assert_eq!(load(&e, "http://site.test", BROKEN), "SyntaxError");
}

#[test]
fn dropping_all_code_drops_precompiled_modules() {
    if !Engine::has_module_precompile() {
        return;
    }
    let e = Engine::new().expect("engine");
    assert_eq!(precompile(&e, "http://site.test", GRAPH, true), GRAPH.len());
    // An idle process gives its code up (`settle_idle`); loads then parse.
    e.settle_idle();
    assert_eq!(e.module_precompile_stats().modules, 0);
    let before = e.module_precompile_stats();
    assert_eq!(load(&e, "http://site.test", GRAPH), EXPECTED);
    let stats = gained(&e, before);
    assert_eq!((stats.record_hits, stats.code_hits), (0, 0), "{stats:?}");
}

#[test]
fn coverage_counts_modules_that_were_precompiled_without_it() {
    if !Engine::has_module_precompile() || !Engine::has_coverage() {
        return;
    }
    let e = Engine::new().expect("engine");
    assert_eq!(precompile(&e, "http://site.test", GRAPH, true), GRAPH.len());
    // Code compiled without block counters cannot count: turning coverage on
    // drops it, and the load below compiles with counters.
    e.set_coverage(true).unwrap();
    assert_eq!(load(&e, "http://site.test", GRAPH), EXPECTED);
    assert_eq!(e.module_precompile_stats().record_hits, 0);
    let scripts = e.new_scripts();
    assert!(
        scripts.iter().any(|s| s.url == "http://site.test/app/lib.js"),
        "{:?}",
        scripts.iter().map(|s| &s.url).collect::<Vec<_>>()
    );
    e.set_coverage(false).unwrap();
}
