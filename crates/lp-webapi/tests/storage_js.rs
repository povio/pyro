//! Web Storage as the WPT webstorage suite sees it: named-property [[Set]] and
//! [[Delete]] semantics, symbol expandos, enumeration, lone surrogates and the
//! `QuotaExceededError` class.

use crate::support::network::Env;
use lp_webapi::{StorageNamespace, StorageSeed};

fn env() -> Env {
    Env::new("http://storage.test/")
}

/// Runs `source` against both storage areas; each run clears its area first.
fn both(env: &Env, source: &str) -> Vec<String> {
    ["localStorage", "sessionStorage"]
        .iter()
        .map(|name| env.eval(&format!("(function(storage){{storage.clear();{source}}})(globalThis.{name})")))
        .collect()
}

#[test]
fn set_invokes_the_named_setter_for_names_the_prototype_chain_has() {
    let env = env();
    for key in ["9", "'x'"] {
        // Data property on Storage.prototype: the setter stores, the getter stays hidden.
        for result in both(
            &env,
            &format!(
                r#"const key = {key}; Storage.prototype[key] = 'proto';
                   try {{
                     const before = storage[key] + '|' + storage.getItem(key);
                     const assigned = (storage[key] = 'value');
                     return before + '|' + assigned + '|' + storage[key] + '|' + Object.getOwnPropertyDescriptor(storage, key) + '|' + storage.getItem(key);
                   }} finally {{ delete Storage.prototype[key]; }}"#
            ),
        ) {
            assert_eq!(result, "proto|null|value|proto|undefined|value", "key {key}");
        }
        // The same with an existing item, and an accessor whose setter must not run.
        for result in both(
            &env,
            &format!(
                r#"const key = {key}; let setterRan = false;
                   Object.defineProperty(Storage.prototype, key, {{ get() {{ return 'proto'; }}, set() {{ setterRan = true; }}, configurable: true }});
                   try {{
                     storage.setItem(key, 'existing');
                     const before = storage[key] + '|' + storage.getItem(key) + '|' + Object.getOwnPropertyDescriptor(storage, key);
                     storage[key] = 'value';
                     return before + '|' + storage[key] + '|' + storage.getItem(key) + '|' + setterRan;
                   }} finally {{ delete Storage.prototype[key]; }}"#
            ),
        ) {
            assert_eq!(result, "proto|existing|undefined|proto|value|false", "key {key}");
        }
    }
    // Built-in member names are stored too and never shadow the members.
    for result in both(
        &env,
        r#"storage.length = 5; storage.getItem = 'g';
           return typeof storage.getItem + '|' + storage.length + '|' + storage.getItem('length') + '|' + storage.getItem('getItem');"#,
    ) {
        assert_eq!(result, "function|2|5|g");
    }
}

#[test]
fn deleting_a_prototype_name_or_a_missing_key_runs_the_ordinary_delete() {
    let env = env();
    for result in both(
        &env,
        r#"storage.setItem('x', 'v');
           Storage.prototype.x = 'proto';
           const whileShadowed = delete storage.x;
           const stillStored = storage.getItem('x');
           delete Storage.prototype.x;
           const visible = delete storage.x;
           const again = delete storage.x;
           return whileShadowed + '|' + stillStored + '|' + visible + '|' + ('x' in storage) + '|' + again;"#,
    ) {
        assert_eq!(result, "true|v|true|false|true");
    }
}

#[test]
fn symbol_keyed_properties_are_ordinary_own_properties() {
    let env = env();
    for result in both(
        &env,
        r#"const key = Symbol();
           Object.defineProperty(storage, key, { value: 'test', configurable: false });
           const desc = Object.getOwnPropertyDescriptor(storage, key);
           const deleted = delete storage[key];
           const missing = delete storage[Symbol()];
           const other = Symbol('desc');
           storage[other] = 'expando';
           const expando = storage[other] + '|' + storage.getItem('desc') + '|' + storage.length;
           const removed = delete storage[other];
           return storage[key] + '|' + desc.configurable + '|' + deleted + '|' + storage[key] + '|' + missing + '|' + expando + '|' + removed + '|' + storage[other];"#,
    ) {
        // The first form is the spec's; the second is today's: the JSC C API hands
        // a symbol to the named hooks under its description, so an assignment
        // with a described symbol is stored as that name (a JSC patch would fix it).
        assert!(
            result == "test|false|false|test|true|expando|null|0|true|undefined"
                || result == "test|false|false|test|true|expando|expando|1|true|undefined",
            "{result}"
        );
    }
}

#[test]
fn enumeration_lists_exactly_the_stored_keys_including_numeric_ones() {
    let env = env();
    for result in both(
        &env,
        r#"Storage.prototype.prototypeTestKey = 'prototypeTestValue';
           try {
             storage.setItem('foo', 'bar'); storage.baz = 'quux'; storage.setItem(0, 'alpha'); storage[42] = 'beta';
             const own = []; for (const p in storage) if (storage.hasOwnProperty(p)) own.push(p);
             // Object.values is not checked: JSC gives callback-getter properties
             // DontEnum, which only a JavaScriptCore patch can change.
             return Object.keys(storage).sort() + '|' + own.sort() + '|' + storage.length + '|' + storage.getItem('prototypeTestKey') + '|' + storage.prototypeTestKey;
           } finally { delete Storage.prototype.prototypeTestKey; }"#,
    ) {
        assert_eq!(result, "0,42,baz,foo|0,42,baz,foo|4|null|prototypeTestValue");
    }
}

#[test]
fn lone_surrogates_round_trip_in_keys_and_values() {
    let env = env();
    for result in both(
        &env,
        r#"const codes = s => s.split('').map(c => c.charCodeAt(0).toString(16)).join(',');
           const out = [];
           for (const str of ['\uD800', '\uDBFF', '\uDC00', '\uDFFF', '\uD83Ca', 'a\uDF4D', '🍍']) {
             storage.clear();
             storage[str] = 'user1';
             out.push((str in storage) + ':' + storage.length + ':' + storage.getItem(str) + ':' + storage[str] + ':' + codes(storage.key(0)) + ':' + codes(Object.keys(storage)[0]));
             storage.clear();
             storage.setItem('name', str);
             out.push(codes(storage.getItem('name')) + ':' + codes(storage.name));
             storage.name = str + '!';
             out.push(codes(storage.getItem('name')));
             storage.removeItem(str); storage.removeItem('name');
             out.push(storage.length);
           }
           return out.join('|');"#,
    ) {
        assert_eq!(
            result,
            "true:1:user1:user1:d800:d800|d800:d800|d800,21|0\
             |true:1:user1:user1:dbff:dbff|dbff:dbff|dbff,21|0\
             |true:1:user1:user1:dc00:dc00|dc00:dc00|dc00,21|0\
             |true:1:user1:user1:dfff:dfff|dfff:dfff|dfff,21|0\
             |true:1:user1:user1:d83c,61:d83c,61|d83c,61:d83c,61|d83c,61,21|0\
             |true:1:user1:user1:61,df4d:61,df4d|61,df4d:61,df4d|61,df4d,21|0\
             |true:1:user1:user1:d83c,df4d:d83c,df4d|d83c,df4d:d83c,df4d|d83c,df4d,21|0"
        );
    }
}

#[test]
fn namespace_api_reads_lossily_and_seeds_round_trip() {
    let seed = StorageSeed::new(vec![(
        "http://storage.test/".into(),
        vec![("k".into(), "v".into()), ("k".into(), "v2".into()), ("\u{1F34D}".into(), "pine".into())],
    )])
    .unwrap();
    let namespace = StorageNamespace::seeded(&seed);
    assert_eq!(namespace.get("http://storage.test", "k").as_deref(), Some("v2"));
    assert_eq!(namespace.get("http://storage.test", "\u{1F34D}").as_deref(), Some("pine"));
    assert_eq!(namespace.get("http://storage.test", "missing"), None);
    assert_eq!(namespace.get("http://other.test", "k"), None);

    let env = env();
    env.page.set_storage_namespaces(namespace.clone(), StorageNamespace::default());
    assert_eq!(
        env.eval("localStorage.length + '|' + localStorage.getItem('k') + '|' + localStorage.key(1)"),
        "2|v2|\u{1F34D}"
    );
    env.eval("localStorage.setItem('lone', '\\uD800x')");
    assert_eq!(namespace.get("http://storage.test", "lone").as_deref(), Some("\u{FFFD}x"));
}

#[test]
fn quota_exceeded_error_class_shape() {
    let env = env();
    let checks = [
        ("typeof QuotaExceededError", "function"),
        ("Object.getPrototypeOf(QuotaExceededError) === DOMException", "true"),
        ("Object.getPrototypeOf(QuotaExceededError.prototype) === DOMException.prototype", "true"),
        ("QuotaExceededError.length", "0"),
        ("QuotaExceededError.prototype[Symbol.toStringTag]", "QuotaExceededError"),
        ("Object.prototype.toString.call(new QuotaExceededError())", "[object QuotaExceededError]"),
        ("(() => { const e = new QuotaExceededError(); return [e.name, e.code, e.message, e.requested, e.quota, e instanceof DOMException, e instanceof Error].map(String).join('|') })()", "QuotaExceededError|22||null|null|true|true"),
        ("(() => { const e = new QuotaExceededError('full', { requested: 5, quota: 3 }); return [e.name, e.message, e.requested, e.quota].join('|') })()", "QuotaExceededError|full|5|3"),
        ("new QuotaExceededError('m', { quota: 3 }).requested", "null"),
        ("new QuotaExceededError('m', { requested: '7' }).requested", "7"),
        ("new QuotaExceededError('m', null).quota", "null"),
        ("try { new QuotaExceededError('m', { requested: -1 }); 'no' } catch (e) { e.constructor.name }", "RangeError"),
        ("try { new QuotaExceededError('m', { quota: -1 }); 'no' } catch (e) { e.constructor.name }", "RangeError"),
        ("try { new QuotaExceededError('m', { requested: 1, quota: 2 }); 'no' } catch (e) { e.constructor.name }", "RangeError"),
        ("try { new QuotaExceededError('m', { requested: NaN }); 'no' } catch (e) { e.constructor.name }", "TypeError"),
        ("try { new QuotaExceededError('m', { quota: Infinity }); 'no' } catch (e) { e.constructor.name }", "TypeError"),
        ("try { new QuotaExceededError('m', 5); 'no' } catch (e) { e.constructor.name }", "TypeError"),
        ("try { Object.getOwnPropertyDescriptor(QuotaExceededError.prototype, 'quota').get.call({}); 'no' } catch (e) { e.constructor.name }", "TypeError"),
        // The legacy spelling stays a plain DOMException.
        ("new DOMException('m', 'QuotaExceededError').constructor === DOMException", "true"),
        ("Object.getOwnPropertyDescriptor(globalThis, 'QuotaExceededError').enumerable", "false"),
    ];
    for (source, expected) in checks {
        assert_eq!(env.eval(source), expected, "{source}");
    }
}

#[test]
fn exceeding_a_storage_quota_throws_a_quota_exceeded_error_per_area() {
    let env = env();
    let fill = |storage: &str| {
        env.eval(&format!(
            r#"(function() {{
                 const storage = globalThis.{storage}; storage.clear();
                 const val = 'x'.repeat(1024); let index = 0;
                 try {{ while (true) {{ index++; storage.setItem('name' + index, val + index); }} }}
                 catch (e) {{
                   return [e.constructor === QuotaExceededError, e instanceof DOMException, e.name, e.code, e.requested, e.quota,
                           'requested' in e, 'quota' in e, index > 1000, storage.getItem('name1') !== null].map(String).join('|');
                 }}
               }})()"#
        ))
    };
    assert_eq!(fill("localStorage"), "true|true|QuotaExceededError|22|null|null|true|true|true|true");
    // The session area has its own quota and the exhausted local area is unaffected until cleared.
    assert_eq!(fill("sessionStorage"), "true|true|QuotaExceededError|22|null|null|true|true|true|true");
    assert_eq!(
        env.eval("try { localStorage.setItem('more', 'x'.repeat(4096)); 'stored' } catch (e) { e.name }"),
        "QuotaExceededError"
    );
    env.eval("localStorage.clear(); sessionStorage.clear()");
    assert_eq!(env.eval("localStorage.setItem('more', 'x'.repeat(4096)); localStorage.length"), "1");
    // A page script replacing the global binding does not change what native code throws.
    env.eval("localStorage.clear(); globalThis.QuotaExceededError = function Fake() {}");
    assert_eq!(fill("localStorage"), "false|true|QuotaExceededError|22|undefined|undefined|false|false|true|true");
}
