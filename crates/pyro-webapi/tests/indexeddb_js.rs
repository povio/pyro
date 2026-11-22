//! IndexedDB on the real JavaScriptCore with a queued browser task loop:
//! requests, transactions (auto-commit, abort and rollback), upgrades and
//! version change across documents, keys, key paths, key generators, indexes
//! and cursors, and the context partition shared across navigations.
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use pyro_dom::parse::parse_document;
use pyro_dom::Dom;
use pyro_js::{ClassDef, Engine, JsClass, Realm, RealmOptions};
use pyro_webapi::*;

struct WinData {
    page: Rc<PageDom>,
    target: EventTargetData,
}
impl WindowHost for WinData {
    fn page(&self) -> &Rc<PageDom> {
        &self.page
    }
    fn event_target(&self) -> &EventTargetData {
        &self.target
    }
}
struct Win;
impl JsClass for Win {
    const NAME: &'static str = "Window";
    type Data = WinData;
    type Parent = EventTarget;
    fn define(def: &mut ClassDef<Self>) {
        def.upcast(|d: &WinData| &d.target);
        define_members::<Win>(def);
    }
}

/// One browser thread's task queue, shared by the documents of a test.
#[derive(Default)]
struct Tasks(RefCell<VecDeque<Box<dyn FnOnce()>>>);
struct Hooks(Rc<Tasks>);
impl PageHooks for Hooks {
    fn post_task(&self, task: Box<dyn FnOnce()>) {
        self.0 .0.borrow_mut().push_back(task);
    }
    fn navigate(&self, _: NavigationRequest) {}
}
impl Tasks {
    fn drain(&self) {
        for _ in 0..100_000 {
            let task = self.0.borrow_mut().pop_front();
            let Some(task) = task else { return };
            task();
        }
        panic!("task loop did not settle");
    }
}

struct Doc {
    page: Rc<PageDom>,
    realm: Realm,
}

fn doc(engine: &Engine, tasks: &Rc<Tasks>, url: &str, local: &StorageNamespace) -> Doc {
    let dom = Rc::new(RefCell::new(Dom::new()));
    let document = parse_document(&dom, url, "<body>");
    dom.borrow_mut().set_document_active(document, true).unwrap();
    let page = PageDom::new(dom, document);
    page.set_storage_namespaces(local.clone(), StorageNamespace::default());
    page.set_hooks(Some(Rc::new(Hooks(tasks.clone()))));
    let class = engine.register::<Win>();
    let realm = engine.create_realm(RealmOptions { global_class: Some(class), ..Default::default() }).unwrap();
    let key = page.create_window_target(document);
    realm.set_global_data::<Win>(WinData { page: page.clone(), target: EventTargetData::window(&page, key) }).unwrap();
    {
        let s = realm.scope().unwrap();
        install(&s, page.clone()).unwrap();
    }
    Doc { page, realm }
}

impl Doc {
    fn eval(&self, src: &str) -> String {
        let s = self.realm.scope().unwrap();
        match s.eval(src, Some("idb-test.js"), 1) {
            Ok(v) => v.to_string().unwrap(),
            Err(e) => format!("THROW {}", e.details(&s).text()),
        }
    }
}

struct Env {
    engine: Engine,
    tasks: Rc<Tasks>,
    local: StorageNamespace,
}

impl Env {
    fn new() -> Env {
        let engine = Engine::new().unwrap();
        register(&engine);
        Env { engine, tasks: Rc::default(), local: StorageNamespace::default() }
    }
    fn doc(&self, url: &str) -> Doc {
        doc(&self.engine, &self.tasks, url, &self.local)
    }
    /// Runs `script` (which sets `globalThis.out`), drains tasks, returns `out`.
    fn run(&self, d: &Doc, script: &str) -> String {
        let started = d.eval(&format!(
            "globalThis.out='pending';(async()=>{{{script}}})().catch(e=>{{out='REJECT '+e.name+': '+e.message}})"
        ));
        assert!(!started.starts_with("THROW"), "{started}");
        self.tasks.drain();
        d.eval("String(out)")
    }
}

/// Promise helpers in the style of the `idb` library.
const HELPERS: &str = r#"
  globalThis.req = r => new Promise((ok, no) => { r.onsuccess = () => ok(r.result); r.onerror = () => no(r.error); });
  globalThis.done = tx => new Promise((ok, no) => { tx.oncomplete = () => ok('complete'); tx.onabort = () => no(tx.error || new DOMException('aborted', 'AbortError')); });
  globalThis.openDb = (name, version, upgrade) => new Promise((ok, no) => {
    const r = indexedDB.open(name, version);
    r.onupgradeneeded = e => upgrade && upgrade(r.result, r.transaction, e);
    r.onsuccess = () => ok(r.result);
    r.onerror = () => no(r.error);
    r.onblocked = () => (globalThis.blocked = (globalThis.blocked || 0) + 1);
  });
"#;

#[test]
fn records_survive_navigation_and_appear_in_databases() {
    let env = Env::new();
    let a = env.doc("http://idb.test/a");
    a.eval(HELPERS);
    let got = env.run(
        &a,
        r#"
        const log = [];
        const db = await openDb('app', 2, (db, tx, e) => {
          log.push(`upgrade ${e.oldVersion}->${e.newVersion} ${e instanceof IDBVersionChangeEvent}`);
          const s = db.createObjectStore('kv');
          s.put({n: 1, when: new Date(5)}, 'one');
          s.put([1, 2], 'two');
        });
        log.push(db.version, [...db.objectStoreNames].join());
        const tx = db.transaction('kv', 'readwrite');
        tx.objectStore('kv').put('three', 3);
        log.push(await done(tx));
        const v = await req(db.transaction('kv').objectStore('kv').get('one'));
        log.push(v.n, v.when instanceof Date && +v.when);
        log.push(JSON.stringify(await req(db.transaction('kv').objectStore('kv').getAllKeys())));
        db.close();
        out = log.join('|');
    "#,
    );
    assert_eq!(got, "upgrade 0->2 true|2|kv|complete|1|5|[3,\"one\",\"two\"]");
    // A navigation: a new document of the same origin and context.
    a.page.dispose();
    let b = env.doc("http://idb.test/b");
    b.eval(HELPERS);
    let got = env.run(
        &b,
        r#"
        const list = await indexedDB.databases();
        const db = await openDb('app');
        const all = await req(db.transaction('kv').objectStore('kv').getAll());
        out = JSON.stringify(list) + '|' + db.version + '|' + JSON.stringify(all.map(v => v.n ?? v));
    "#,
    );
    assert_eq!(got, r#"[{"name":"app","version":2}]|2|["three",1,[1,2]]"#);
    // Other origins and contexts do not see it.
    let other = env.doc("http://other.test/");
    assert_eq!(env.run(&other, "out = JSON.stringify(await indexedDB.databases())"), "[]");
    let isolated = doc(&env.engine, &env.tasks, "http://idb.test/", &StorageNamespace::default());
    assert_eq!(env.run(&isolated, "out = (await indexedDB.databases()).length"), "0");
}

#[test]
fn databases_exposes_only_committed_upgrades() {
    let env = Env::new();
    let doc = env.doc("http://idb.test/");
    doc.eval(HELPERS);
    let got = env.run(
        &doc,
        r#"
        const catalog = () => indexedDB.databases().then(rows => rows.map(row => `${row.name}:${row.version}`).sort().join(','));
        const stable = await openDb('stable', 1);
        const log = [];
        let snapshot;
        let db = await openDb('changing', 1, conn => {
          conn.createObjectStore('rows').put('kept', 1);
          snapshot = catalog();
        });
        log.push(await snapshot, await catalog());
        db.close();
        db = await openDb('changing', 2, () => { snapshot = catalog(); });
        log.push(await snapshot, await catalog());
        db.close();
        const failure = await openDb('changing', 3, (conn, tx) => {
          snapshot = catalog();
          conn.deleteObjectStore('rows');
          tx.abort();
        }).catch(error => error.name);
        log.push(await snapshot, failure, await catalog());
        db = await openDb('changing');
        log.push(db.version, await req(db.transaction('rows').objectStore('rows').get(1)));
        db.close(); stable.close();
        out = log.join('|');
    "#,
    );
    assert_eq!(
        got,
        "stable:1|changing:1,stable:1|changing:1,stable:1|changing:2,stable:1|changing:2,stable:1|AbortError|changing:2,stable:1|2|kept"
    );
}

#[test]
fn keys_order_compare_and_validate() {
    let env = Env::new();
    let d = env.doc("http://idb.test/");
    let got = d.eval(
        r#"(() => {
          const keys = [[1], new Uint8Array([1, 2]).buffer, 'b', 'a', new Date(3), Infinity, -Infinity, 0, -1.5, [0, 'x'], [], ''];
          const sorted = keys.slice().sort((a, b) => indexedDB.cmp(a, b));
          const show = k => k instanceof ArrayBuffer ? 'bin' : k instanceof Date ? 'date' : Array.isArray(k) ? '[' + k.map(show) + ']' : typeof k === 'string' ? JSON.stringify(k) : String(k);
          const err = f => { try { f(); return 'ok'; } catch (e) { return e.name; } };
          return [sorted.map(show).join(' '), indexedDB.cmp(-0, 0), indexedDB.cmp([1, [2]], [1, [2]]),
            err(() => indexedDB.cmp(NaN, 1)), err(() => indexedDB.cmp({}, 1)), err(() => indexedDB.cmp(new Date(NaN), 1)),
            err(() => { const a = []; a.push(a); indexedDB.cmp(a, 1); }), err(() => indexedDB.cmp(1)),
            indexedDB.cmp('\uD800', '￿'), indexedDB.cmp('a', 'a\u0000')].join('|');
        })()"#,
    );
    assert_eq!(got, "-Infinity -1.5 0 Infinity date \"\" \"a\" \"b\" bin [] [0,\"x\"] [1]|0|0|DataError|DataError|DataError|DataError|TypeError|-1|-1");
    let got = d.eval(
        r#"(() => {
          const r = IDBKeyRange.bound(1, 5, true, false);
          const err = f => { try { f(); return 'ok'; } catch (e) { return e.name; } };
          return [r.lower, r.upper, r.lowerOpen, r.upperOpen, r.includes(1), r.includes(5), r.includes(3),
            IDBKeyRange.only('x').includes('x'), IDBKeyRange.lowerBound(2).upperOpen, IDBKeyRange.upperBound(2).lowerOpen,
            err(() => IDBKeyRange.bound(5, 1)), err(() => IDBKeyRange.bound(1, 1, true)), err(() => IDBKeyRange.only({})),
            err(() => new IDBKeyRange()), Object.prototype.toString.call(r)].join();
        })()"#,
    );
    assert_eq!(
        got,
        "1,5,true,false,false,true,true,true,true,true,DataError,DataError,DataError,TypeError,[object IDBKeyRange]"
    );
}

#[test]
fn key_paths_generators_and_constraints() {
    let env = Env::new();
    let d = env.doc("http://idb.test/");
    d.eval(HELPERS);
    let got = env.run(
        &d,
        r#"
        const log = [];
        const err = f => { try { f(); return 'ok'; } catch (e) { return e.name; } };
        const db = await openDb('paths', 1, db => {
          const s = db.createObjectStore('auto', {keyPath: 'a.b.id', autoIncrement: true});
          log.push(s.keyPath, s.autoIncrement);
          db.createObjectStore('compound', {keyPath: ['x', 'y']});
          db.createObjectStore('gen', {autoIncrement: true});
          log.push(err(() => db.createObjectStore('bad', {keyPath: 'not valid'})),
            err(() => db.createObjectStore('auto')),
            err(() => db.createObjectStore('bad2', {keyPath: '', autoIncrement: true})));
        });
        const tx = db.transaction(['auto', 'compound', 'gen'], 'readwrite');
        const auto = tx.objectStore('auto'), gen = tx.objectStore('gen'), compound = tx.objectStore('compound');
        log.push(await req(auto.put({name: 'n'})), await req(auto.put({a: {b: {id: 10}}})), await req(auto.add({})));
        log.push(err(() => auto.put({a: 1})), err(() => auto.put({}, 1)), err(() => compound.put({x: 1})));
        log.push(JSON.stringify(await req(compound.put({x: 1, y: 'z'}))));
        log.push(await req(gen.put('a')), await req(gen.put('b', 7.5)), await req(gen.put('c')));
        const dup = gen.add('d', 1);
        dup.onerror = e => { log.push(dup.error.name); e.preventDefault(); };
        log.push(JSON.stringify((await req(auto.get(1)))), await done(tx));
        out = log.join('|');
    "#,
    );
    assert_eq!(
        got,
        "a.b.id|true|SyntaxError|ConstraintError|InvalidAccessError|1|10|11|DataError|DataError|DataError|[1,\"z\"]|1|7.5|8|ConstraintError|{\"name\":\"n\",\"a\":{\"b\":{\"id\":1}}}|complete"
    );
}

#[test]
fn terminal_events_precede_waiting_transaction_events() {
    let env = Env::new();
    let doc = env.doc("http://idb.test/");
    doc.eval(HELPERS);
    let got = env.run(
        &doc,
        r#"
        const db = await openDb('terminal-order', 1, db => db.createObjectStore('rows'));
        const events = [];
        for (const abort of [false, true]) {
          const writer = db.transaction('rows', 'readwrite');
          const terminal = done(writer).then(() => events.push('commit'), e => events.push(e.name));
          const rows = writer.objectStore('rows');
          await req(rows.put(abort ? 'temporary' : 'saved', 1));
          const first = db.transaction('rows');
          const empty = done(first).then(() => events.push('empty'));
          const second = db.transaction('rows');
          const read = req(second.objectStore('rows').get(1)).then(value => events.push(value));
          const finished = done(second);
          if (abort) {
            const duplicate = rows.add('bad', 1);
            duplicate.onerror = () => events.push('error');
          } else {
            await req(rows.put('also saved', 2));
          }
          await Promise.all([terminal, empty, read, finished]);
          const successors = events.splice(-2).sort();
          events.push(...successors);
        }
        db.close();
        const reopened = await openDb('terminal-order', 1);
        events.push(await req(reopened.transaction('rows').objectStore('rows').count()));
        out = events.join('|');
        "#,
    );
    assert_eq!(got, "commit|empty|saved|error|ConstraintError|empty|saved|2");
}

#[test]
fn array_key_conversion_rejects_proxies_without_traps() {
    let env = Env::new();
    let doc = env.doc("http://idb.test/");
    doc.eval(HELPERS);
    let got = env.run(
        &doc,
        r#"
        const db = await openDb('array-key-brand', 1, db => db.createObjectStore('rows'));
        const tx = db.transaction('rows', 'readwrite');
        const completed = done(tx);
        const rows = tx.objectStore('rows');
        let traps = 0, reads = 0;
        const trap = () => { traps++; throw new Error('unexpected trap'); };
        const proxy = new Proxy([1], {get: trap, getOwnPropertyDescriptor: trap});
        const revoked = Proxy.revocable([], {});
        revoked.revoke();
        const error = operation => { try { operation(); return 'accepted'; } catch (e) { return e.name; } };
        const failures = [error(() => rows.add('bad', proxy)), error(() => indexedDB.cmp([proxy], [])),
                          error(() => IDBKeyRange.only(revoked.proxy))];
        class Keys extends Array {}
        const key = new Keys();
        Object.defineProperty(key, 0, {get() { reads++; return 17; }});
        await req(rows.add('stored', key));
        await completed;
        db.close();
        const reopened = await openDb('array-key-brand', 1);
        const stored = await req(reopened.transaction('rows').objectStore('rows').get([17]));
        out = [...failures, traps, reads, stored].join('|');
        "#,
    );
    assert_eq!(got, "DataError|DataError|DataError|0|1|stored");
}

#[test]
fn decoded_keys_bypass_inherited_index_setters() {
    let env = Env::new();
    let doc = env.doc("http://idb.test/");
    doc.eval(HELPERS);
    let got = env.run(
        &doc,
        r#"
        const db = await openDb('returned-keys', 1, db => db.createObjectStore('rows'));
        const tx = db.transaction('rows', 'readwrite');
        const completed = done(tx);
        const rows = tx.objectStore('rows');
        const key = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, ['abcdefghijk', Uint8Array.of(0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 255)]];
        let calls = 0;
        Object.defineProperty(Object.prototype, '10', {configurable: true, set() { calls++; }});
        try {
          const result = await req(rows.put('saved', key));
          out = [calls, Object.hasOwn(result, 10), indexedDB.cmp(result, key)].join('|');
        } finally {
          delete Object.prototype[10];
        }
        await completed;
        db.close();
        const reopened = await openDb('returned-keys', 1);
        out += '|' + await req(reopened.transaction('rows').objectStore('rows').get(key));
        "#,
    );
    assert_eq!(got, "0|true|0|saved");
}

#[test]
fn blob_file_key_paths_use_stored_metadata() {
    let env = Env::new();
    let doc = env.doc("http://idb.test/");
    doc.eval(HELPERS);
    let got = env.run(
        &doc,
        r#"
        const db = await openDb('file-keys', 1, db => {
          const store = db.createObjectStore('rows', {keyPath: ['payload.size', 'payload.type']});
          store.createIndex('files', ['payload.name', 'payload.lastModified']);
        });
        const tx = db.transaction('rows', 'readwrite');
        const rows = tx.objectStore('rows');
        const completed = done(tx);
        const blobKey = await req(rows.add({payload: new Blob([])}));
        const fileKey = await req(rows.add({payload: new File(['ab'], 'record', {type: 'text/plain', lastModified: 0})}));
        await completed;
        const aborted = db.transaction('rows', 'readwrite');
        const rolledBack = done(aborted).catch(error => error.name);
        await req(aborted.objectStore('rows').delete(fileKey));
        aborted.abort();
        await rolledBack;
        db.close();
        const reopened = await openDb('file-keys', 1);
        const index = reopened.transaction('rows').objectStore('rows').index('files');
        const count = await req(index.count());
        const restored = await req(index.get(['record', 0]));
        out = JSON.stringify([blobKey, fileKey, count, restored.payload.name, restored.payload.size]);
        "#,
    );
    assert_eq!(got, r#"[[0,""],[2,"text/plain"],1,"record",2]"#);
}

#[test]
fn failed_unique_writes_preserve_the_generator() {
    let env = Env::new();
    let doc = env.doc("http://idb.test/");
    doc.eval(HELPERS);
    let got = env.run(
        &doc,
        r#"
        const db = await openDb('failed-writes', 1, db => {
          db.createObjectStore('rows', {autoIncrement: true}).createIndex('tag', 'tag', {unique: true});
        });
        const tx = db.transaction('rows', 'readwrite');
        const rows = tx.objectStore('rows');
        const canceled = request => new Promise(resolve => {
          request.onerror = event => { event.preventDefault(); resolve(request.error.name); };
          request.onsuccess = () => resolve('unexpected success');
        });
        await req(rows.add({tag: 'original'}));
        const generatedError = await canceled(rows.add({tag: 'original'}));
        const afterGenerated = await req(rows.add({tag: 'second'}));
        const explicitError = await canceled(rows.put({tag: 'original'}, 2 ** 53));
        const afterExplicit = await req(rows.add({tag: 'third'}));
        const explicit = await req(rows.put({tag: 'explicit'}, 10.5));
        const advanced = await req(rows.add({tag: 'advanced'}));
        await done(tx);
        const aborted = db.transaction('rows', 'readwrite');
        const abortResult = done(aborted).catch(error => error.name);
        const tentative = aborted.objectStore('rows');
        await req(tentative.put({tag: 'temporary'}, 100));
        tentative.add({tag: 'original'});
        const abortError = await abortResult;
        db.close();
        const reopened = await openDb('failed-writes', 1);
        const nextTx = reopened.transaction('rows', 'readwrite');
        const nextRows = nextTx.objectStore('rows');
        const count = await req(nextRows.count());
        const rolledBack = await req(nextRows.get(100)) === undefined;
        const next = await req(nextRows.add({tag: 'reopened'}));
        out = [generatedError, afterGenerated, explicitError, afterExplicit, explicit, advanced,
               abortError, count, rolledBack, next, await done(nextTx)].join('|');
        "#,
    );
    assert_eq!(got, "ConstraintError|2|ConstraintError|3|10.5|11|ConstraintError|5|true|12|complete");
}

#[test]
fn key_generator_exhausts_after_2_pow_53() {
    let env = Env::new();
    let d = env.doc("http://idb.test/");
    d.eval(HELPERS);
    let got = env.run(
        &d,
        r#"
        const log = [];
        const db = await openDb('overflow', 1, db => {
          db.createObjectStore('bumped', {autoIncrement: true});
          db.createObjectStore('explicit', {autoIncrement: true});
        });
        const tx = db.transaction(['bumped', 'explicit'], 'readwrite');
        const fails = r => new Promise(ok => { r.onsuccess = () => ok('stored ' + r.result); r.onerror = e => { e.preventDefault(); ok(r.error.name); }; });
        const bumped = tx.objectStore('bumped'), explicit = tx.objectStore('explicit');
        // put() would silently overwrite 2^53 if the generator stayed there.
        bumped.put('a', 9007199254740991);
        log.push(await req(bumped.put('b')), await fails(bumped.put('c')));
        explicit.put('a', 9007199254740992);
        log.push(await fails(explicit.put('b')), await fails(explicit.put('c')));
        log.push(await req(bumped.count()), await done(tx));
        out = log.join('|');
    "#,
    );
    assert_eq!(got, "9007199254740992|ConstraintError|ConstraintError|ConstraintError|2|complete");
}

#[test]
fn request_callback_exception_respects_explicit_commit() {
    let env = Env::new();
    let doc = env.doc("http://idb.test/");
    doc.eval(HELPERS);
    let got = env.run(
        &doc,
        r#"
        let db = await openDb('callbacks', 1, db => db.createObjectStore('rows'));
        const log = [];
        for (const mode of ['before', 'inside', 'implicit', 'duplicate']) {
          const tx = db.transaction('rows', 'readwrite');
          const rows = tx.objectStore('rows');
          const first = rows.add(mode, mode);
          if (mode === 'duplicate') rows.add('replacement', 'before');
          else first.onsuccess = () => {
            if (mode === 'inside') tx.commit();
            throw new Error('listener');
          };
          if (mode === 'before' || mode === 'duplicate') tx.commit();
          log.push(await done(tx).catch(error => error.name));
        }
        db.close();
        db = await openDb('callbacks');
        log.push((await req(db.transaction('rows').objectStore('rows').getAll())).join(','));
        db.close();
        out = log.join('|');
    "#,
    );
    assert_eq!(got, "complete|complete|AbortError|ConstraintError|before,inside");
}

#[test]
fn aborts_roll_back_data_generators_and_upgrades() {
    let env = Env::new();
    let d = env.doc("http://idb.test/");
    d.eval(HELPERS);
    let got = env.run(
        &d,
        r#"
        const log = [];
        let db = await openDb('roll', 1, db => db.createObjectStore('s', {autoIncrement: true}));
        await done((() => { const tx = db.transaction('s', 'readwrite'); tx.objectStore('s').put('kept'); return tx; })());
        const tx = db.transaction('s', 'readwrite');
        const s = tx.objectStore('s');
        s.put('lost'); s.put('lost2');
        s.delete(1).onsuccess = () => tx.abort();
        log.push(await done(tx).catch(e => e.name + ':' + tx.error));
        const r = db.transaction('s').objectStore('s');
        log.push(JSON.stringify(await req(r.getAll())));
        const t2 = db.transaction('s', 'readwrite');
        log.push(await req(t2.objectStore('s').put('next')));
        db.close();
        // An aborted upgrade restores the version and store set.
        const open = indexedDB.open('roll', 2);
        open.onupgradeneeded = () => { open.result.createObjectStore('extra'); open.result.deleteObjectStore('s'); open.transaction.abort(); };
        log.push(await req(open).catch(e => e.name + ':' + open.result + ':' + open.transaction));
        db = await openDb('roll');
        log.push(db.version, [...db.objectStoreNames].join(), (await req(db.transaction('s').objectStore('s').count())));
        db.close();
        // An aborted creation leaves no database.
        const fresh = indexedDB.open('never', 3);
        fresh.onupgradeneeded = () => { throw new Error('boom'); };
        log.push(await req(fresh).catch(e => e.name), JSON.stringify((await indexedDB.databases()).map(d => d.name)));
        out = log.join('|');
    "#,
    );
    assert_eq!(got, "AbortError:null|[\"kept\"]|2|AbortError:undefined:null|1|s|2|AbortError|[\"roll\"]");
}

#[test]
fn aborted_clear_restores_records_indexes_and_generator() {
    let env = Env::new();
    let d = env.doc("http://idb.test/");
    d.eval(HELPERS);
    let got = env.run(
        &d,
        r#"
        const db = await openDb('clear', 1, db => db.createObjectStore('s', {keyPath: 'id', autoIncrement: true}).createIndex('v', 'v', {unique: true}));
        const fill = db.transaction('s', 'readwrite');
        fill.objectStore('s').put({v: 'x'}); fill.objectStore('s').put({v: 'y'});
        await done(fill);
        const tx = db.transaction('s', 'readwrite');
        const s = tx.objectStore('s');
        s.clear();
        const during = [await req(s.count()), await req(s.index('v').count()), await req(s.put({v: 'x'}))];
        tx.abort();
        const r = db.transaction('s', 'readwrite').objectStore('s');
        out = [during.join(), JSON.stringify(await req(r.index('v').getAllKeys())), await req(r.put({v: 'z'}))].join('|');
    "#,
    );
    // The index kept its entries and its uniqueness, the generator went back to 3.
    assert_eq!(got, "0,0,3|[1,2]|3");
}

#[test]
fn indexes_cursors_and_directions() {
    let env = Env::new();
    let d = env.doc("http://idb.test/");
    d.eval(HELPERS);
    let got = env.run(
        &d,
        r#"
        const log = [];
        const db = await openDb('ix', 1, db => {
          const s = db.createObjectStore('people', {keyPath: 'id'});
          s.createIndex('byAge', 'age');
          s.createIndex('byTag', 'tags', {multiEntry: true});
          s.createIndex('byMail', 'mail', {unique: true});
          for (const [id, age, tags, mail] of [[1, 30, ['a', 'b', 'a'], 'x'], [2, 20, ['b'], 'y'], [3, 30, [], 'z'], [4, 40, 'c', 'w']])
            s.put({id, age, tags, mail});
        });
        const tx = db.transaction('people', 'readwrite');
        const s = tx.objectStore('people');
        log.push([...s.indexNames].join());
        const byAge = s.index('byAge');
        log.push(JSON.stringify(await req(byAge.getAllKeys(30))), await req(byAge.count(IDBKeyRange.lowerBound(30))));
        log.push(JSON.stringify(await req(s.index('byTag').getAllKeys('b'))), JSON.stringify(await req(s.index('byTag').getAllKeys())));
        log.push((await req(byAge.get(IDBKeyRange.bound(25, 35)))).id, await req(byAge.getKey(40)));
        for (const dir of ['next', 'nextunique', 'prev', 'prevunique']) {
          const seen = [];
          await new Promise(ok => {
            const c = byAge.openCursor(null, dir);
            c.onsuccess = () => { const cur = c.result; if (!cur) return ok(); seen.push(cur.key + ':' + cur.primaryKey); cur.continue(); };
          });
          log.push(dir + '=' + seen.join(','));
        }
        // continuePrimaryKey, advance, update and delete through cursors.
        await new Promise(ok => {
          let first = true;
          const c = byAge.openCursor();
          c.onsuccess = () => {
            const cur = c.result;
            if (!cur) return ok();
            if (first) { first = false; return cur.continuePrimaryKey(30, 3); }
            log.push('cpk=' + cur.primaryKey + ':' + (cur.source === byAge) + ':' + (cur.request === c));
            ok();
          };
        });
        await new Promise(ok => {
          const c = s.openCursor(IDBKeyRange.lowerBound(1, true));
          let n = 0;
          c.onsuccess = () => {
            const cur = c.result;
            if (!cur) return ok();
            if (n++ === 0) { cur.update({...cur.value, age: 99}).onsuccess = () => cur.advance(2); }
            else { log.push('adv=' + cur.key); cur.delete().onsuccess = () => ok(); }
          };
        });
        log.push(JSON.stringify(await req(byAge.getAllKeys())));
        const bad = s.put({id: 9, age: 1, mail: 'x'});
        bad.onerror = e => { log.push('unique=' + bad.error.name); e.preventDefault(); };
        log.push(JSON.stringify((await req(s.getAllRecords({direction: 'prev', count: 2}))).map(r => [r.key, r.primaryKey, r.value.id])));
        log.push(JSON.stringify(await req(s.getAll({query: IDBKeyRange.upperBound(3), count: 5}))).length > 2);
        log.push(await done(tx));
        out = log.join('|');
    "#,
    );
    assert_eq!(
        got,
        "byAge,byMail,byTag|[1,3]|3|[1,2]|[1,1,2,4]|1|4|next=20:2,30:1,30:3,40:4|nextunique=20:2,30:1,40:4|prev=40:4,30:3,30:1,20:2|prevunique=40:4,30:1,20:2|cpk=3:true:true|adv=4|[1,3,2]|unique=ConstraintError|[[3,3,3],[2,2,2]]|true|complete"
    );
}

#[test]
fn requests_events_and_transaction_lifetime() {
    let env = Env::new();
    let d = env.doc("http://idb.test/");
    d.eval(HELPERS);
    let got = env.run(
        &d,
        r#"
        const log = [];
        const db = await openDb('ev', 1, db => db.createObjectStore('s'));
        // Error events bubble request -> transaction -> database; capture runs first.
        db.addEventListener('error', e => log.push('db-capture:' + e.target.constructor.name), true);
        db.onerror = e => log.push('db-bubble:' + (e.target.transaction === tx));
        const tx = db.transaction('s', 'readwrite');
        tx.onerror = e => log.push('tx:' + e.eventPhase);
        const s = tx.objectStore('s');
        s.add(1, 'k');
        const dup = s.add(2, 'k');
        dup.onerror = e => { log.push('req:' + e.isTrusted + ':' + e.bubbles + ':' + e.cancelable); };
        log.push(dup.readyState);
        let readError; try { dup.result; } catch (e) { readError = e.name; }
        log.push(readError);
        log.push(await done(tx).catch(e => 'abort:' + e.name));
        // await between requests: microtasks run while the transaction is active.
        const t2 = db.transaction('s', 'readwrite');
        const s2 = t2.objectStore('s');
        await req(s2.put('a', 1));
        const v = await req(s2.get(1));
        await req(s2.put(v + 'b', 2));
        log.push(await done(t2));
        out = log.join('|');
    "#,
    );
    assert_eq!(
        got,
        "pending|InvalidStateError|db-capture:IDBRequest|req:true:true:true|tx:3|db-bubble:true|abort:ConstraintError|complete"
    );
    let got = env.run(
        &d,
        r#"
        const log = [];
        const db = await openDb('ev');
        const t = db.transaction('s', 'readwrite');
        const r = t.objectStore('s').put('x', 3);
        r.onsuccess = () => { throw new Error('handler'); };
        log.push(await done(t).catch(e => e.name));
        const rd = db.transaction('s');
        const rs = rd.objectStore('s');
        rs.count();
        await done(rd);
        let late;
        try { rs.get(1); } catch (e) { late = e.name; }
        log.push(late, JSON.stringify(await req(db.transaction('s').objectStore('s').getAll())));
        const ro = db.transaction('s');
        let ro_err; try { ro.objectStore('s').put(1, 1); } catch (e) { ro_err = e.name; }
        log.push(ro_err, ro.mode, ro.durability, db.transaction('s', 'readwrite', {durability: 'relaxed'}).durability);
        const c = db.transaction('s', 'readwrite');
        c.objectStore('s').put('committed', 4);
        c.commit();
        let after; try { c.objectStore('s').put(1, 5); } catch (e) { after = e.name; }
        log.push(after, await done(c));
        out = log.join('|');
    "#,
    );
    assert_eq!(got, "AbortError|TransactionInactiveError|[\"a\",\"ab\"]|ReadOnlyError|readonly|default|relaxed|TransactionInactiveError|complete");
    // Only the IDB event's own listeners abort: an exception in a listener of
    // another event they dispatch is reported there and does not count.
    let got = env.run(
        &d,
        r#"
        const db = await openDb('ev');
        const t = db.transaction('s', 'readwrite');
        const inner = new EventTarget();
        inner.addEventListener('x', () => { throw new Error('inner'); });
        t.objectStore('s').put('y', 6).onsuccess = () => inner.dispatchEvent(new Event('x'));
        out = await done(t).catch(e => 'abort ' + e.name);
    "#,
    );
    assert_eq!(got, "complete");
}

#[test]
fn version_change_blocks_until_other_documents_close() {
    let env = Env::new();
    let a = env.doc("http://idb.test/a");
    let b = env.doc("http://idb.test/b");
    a.eval(HELPERS);
    b.eval(HELPERS);
    assert_eq!(
        env.run(&a, "globalThis.db = await openDb('shared', 1, db => db.createObjectStore('s')); out = db.version"),
        "1"
    );
    // B upgrades: A gets versionchange and does not close; B is blocked.
    a.eval("globalThis.seen = []; db.onversionchange = e => seen.push(e.oldVersion + '->' + e.newVersion);");
    b.eval("globalThis.out = 'pending'; openDb('shared', 2, db => db.createObjectStore('t')).then(db => { out = db.version + ':' + [...db.objectStoreNames]; db.close(); })");
    env.tasks.drain();
    assert_eq!(a.eval("seen.join()"), "1->2");
    assert_eq!(b.eval("out + ':' + blocked"), "pending:1");
    // A closes: B's upgrade proceeds.
    a.eval("db.close()");
    env.tasks.drain();
    assert_eq!(b.eval("out"), "2:s,t");
    // Deletion: versionchange with a null new version, then success.
    let got = env.run(
        &a,
        r#"
        const db = await openDb('shared');
        db.onversionchange = e => { out2.push('vc:' + e.newVersion); db.close(); };
        globalThis.out2 = [];
        const del = indexedDB.deleteDatabase('shared');
        const e = await new Promise(ok => del.onsuccess = ok);
        out = out2.join() + '|' + e.oldVersion + '|' + e.newVersion + '|' + del.result + '|' + (await indexedDB.databases()).length;
    "#,
    );
    assert_eq!(got, "vc:null|2|null|undefined|0");
    // A document going away closes its connections.
    assert_eq!(env.run(&a, "globalThis.keep = await openDb('gone', 1); out = 'open'"), "open");
    a.page.dispose();
    assert_eq!(
        env.run(&b, "const db = await openDb('gone', 2); out = db.version + ':' + (globalThis.blocked || 0)"),
        "2:1"
    );
}

#[test]
fn window_indexeddb_is_an_own_readonly_accessor() {
    let env = Env::new();
    let document = env.doc("http://idb.test/");
    assert_eq!(
        document.eval(
            r#"
            const descriptor = Object.getOwnPropertyDescriptor(globalThis, 'indexedDB');
            const get = descriptor.get;
            let wrongReceiver;
            try { get.call({}); } catch (error) { wrongReceiver = error.name; }
            [!('indexedDB' in Window.prototype), descriptor.enumerable, descriptor.configurable,
             descriptor.set === undefined, get.name, get.length, get.call(undefined) === indexedDB,
             get.call(globalThis) === indexedDB, wrongReceiver, Reflect.set(globalThis, 'indexedDB', 1),
             delete globalThis.indexedDB, !('indexedDB' in globalThis)].join('|')
        "#
        ),
        "true|true|true|true|get indexedDB|0|true|true|TypeError|false|true|true"
    );
}

#[test]
fn interfaces_are_lazy_globals_and_opaque_origins_reject() {
    let env = Env::new();
    let d = env.doc("http://idb.test/");
    assert_eq!(
        d.eval("typeof Object.getOwnPropertyDescriptor(globalThis, 'IDBKeyRange').get"),
        "function",
        "an accessor until first use"
    );
    assert_eq!(
        d.eval("[IDBKeyRange, IDBDatabase, IDBCursorWithValue, IDBVersionChangeEvent, DOMStringList].map(c => typeof c).join() + ':' + (IDBCursorWithValue.prototype instanceof IDBCursor) + ':' + (IDBOpenDBRequest.prototype instanceof IDBRequest) + ':' + (IDBRequest.prototype instanceof EventTarget)"),
        "function,function,function,function,function:true:true:true"
    );
    assert_eq!(d.eval("typeof Object.getOwnPropertyDescriptor(globalThis, 'IDBKeyRange').value"), "function");
    assert_eq!(d.eval("try { new IDBDatabase(); 'no' } catch (e) { e.name }"), "TypeError");
    assert_eq!(d.eval("const e = new IDBVersionChangeEvent('x', {oldVersion: 2}); e.oldVersion + ':' + e.newVersion + ':' + e.type"), "2:null:x");
    assert_eq!(d.eval("try { indexedDB.open('x', 0); } catch (e) { e.name }"), "TypeError");
    assert_eq!(d.eval("try { indexedDB.open('x', -1); } catch (e) { e.name }"), "TypeError");
    let opaque = env.doc("data:text/html,x");
    assert_eq!(opaque.eval("try { indexedDB.open('db'); 'no' } catch (e) { e.name }"), "SecurityError");
    assert_eq!(opaque.eval("try { indexedDB.deleteDatabase('db'); 'no' } catch (e) { e.name }"), "SecurityError");
    assert_eq!(opaque.eval("typeof IDBKeyRange.only(1)"), "object");
    // cmp touches no storage: it works for opaque origins too.
    assert_eq!(opaque.eval("indexedDB.cmp([1], 'a') + ':' + indexedDB.cmp(1, 2)"), "1:-1");
    // The interface tags live on the prototypes, as in Chrome.
    assert_eq!(
        d.eval("const r = indexedDB.open('tags'); Object.getOwnPropertySymbols(r).length + ':' + Object.prototype.toString.call(r)"),
        "0:[object IDBOpenDBRequest]"
    );
}

#[test]
fn worker_idb_interfaces_start_as_own_data_properties() {
    let env = crate::support::network::Env::new("http://idb.test/page");
    env.eval(
        r#"
        const source = `
          const failures = [];
          for (const name of ['IDBCursor', 'IDBCursorWithValue', 'IDBDatabase', 'IDBIndex',
            'IDBKeyRange', 'IDBObjectStore', 'IDBOpenDBRequest', 'IDBRecord', 'IDBRequest',
            'IDBTransaction', 'IDBVersionChangeEvent']) {
            const desc = Object.getOwnPropertyDescriptor(self, name);
            if (!desc || typeof desc.value !== 'function' || desc.value !== self[name] ||
                !desc.writable || desc.enumerable || !desc.configurable || 'get' in desc ||
                Object.hasOwn(WorkerGlobalScope.prototype, name)) failures.push(name);
          }
          const open = indexedDB.open('worker-interface-identity', 1);
          if (Object.getPrototypeOf(open) !== IDBOpenDBRequest.prototype) failures.push('request identity');
          open.onupgradeneeded = () => {
            if (Object.getPrototypeOf(open.transaction) !== IDBTransaction.prototype) failures.push('tx identity');
          };
          open.onerror = () => postMessage(open.error.name);
          open.onsuccess = () => {
            if (Object.getPrototypeOf(open.result) !== IDBDatabase.prototype) failures.push('db identity');
            open.result.close();
            postMessage(failures.join(',') || 'ok');
          };`;
        globalThis.reply = null;
        globalThis.worker = new Worker(URL.createObjectURL(new Blob([source])));
        worker.onmessage = event => { reply = event.data; };
        worker.onerror = event => { reply = 'error: ' + event.message; };
    "#,
    );
    env.until("reply !== null");
    assert_eq!(env.eval("reply"), "ok");
    env.eval("worker.terminate()");
}

#[test]
fn data_worker_uses_opaque_origin_for_indexeddb() {
    let env = crate::support::network::Env::with_engine_origin(
        "http://idb.test/page",
        Rc::new(Engine::new().unwrap()),
        Some("http://idb.test".into()),
    );
    env.eval(
        r#"
        globalThis.reply = null;
        const source = `
          const syncResult = fn => { try { fn(); return 'returned'; } catch (error) { return error.name; } };
          const results = [self.origin, typeof IDBKeyRange, indexedDB.cmp(1, 2),
            syncResult(() => indexedDB.open('worker-db')),
            syncResult(() => indexedDB.deleteDatabase('worker-db'))];
          indexedDB.databases().then(() => results.push('resolved'), error => results.push(error.name))
            .then(() => postMessage(results.join('|')));`;
        globalThis.worker = new Worker('data:text/javascript,' + encodeURIComponent(source));
        worker.onmessage = event => { reply = event.data; };
        worker.onerror = event => { reply = 'error: ' + event.message; };
        "#,
    );
    env.until("reply !== null");
    let reply = env.eval("reply");
    env.eval("worker.terminate()");
    assert_eq!(reply, "null|function|-1|SecurityError|SecurityError|SecurityError");
}

#[test]
fn dedicated_workers_share_the_owner_partition() {
    let env = crate::support::network::Env::new("http://idb.test/page");
    env.eval(
        r#"
        const src = `
          const r = indexedDB.open('from-worker', 3);
          r.onupgradeneeded = () => r.result.createObjectStore('s', {autoIncrement: true});
          r.onerror = () => postMessage('error ' + r.error.name);
          r.onsuccess = () => {
            const tx = r.result.transaction('s', 'readwrite');
            const put = tx.objectStore('s').put({v: 42});
            tx.oncomplete = () => postMessage([typeof IDBKeyRange, self.indexedDB instanceof IDBFactory, put.result].join());
          };`;
        globalThis.msgs = [];
        globalThis.w = new Worker(URL.createObjectURL(new Blob([src])));
        w.onmessage = e => { msgs.push(e.data); indexedDB.databases().then(d => msgs.push(JSON.stringify(d))); };
        w.onerror = e => msgs.push('error:' + e.message);
    "#,
    );
    env.until("msgs.length >= 1");
    env.until("msgs.length >= 2 || msgs[0].startsWith('error')");
    assert_eq!(env.eval("msgs.join('|')"), r#"function,true,1|[{"name":"from-worker","version":3}]"#);
    env.eval("w.terminate()");
}

#[test]
fn open_request_releases_transaction_after_terminal_dispatch() {
    let env = Env::new();
    let doc = env.doc("http://idb.test/");
    for path in ["commit", "abort", "constraint"] {
        doc.eval(&format!("globalThis.path = '{path}'"));
        doc.eval(
            r#"
            globalThis.trace = [];
            globalThis.openRequest = indexedDB.open(path, 1);
            openRequest.onupgradeneeded = () => {
              const tx = openRequest.transaction;
              const store = openRequest.result.createObjectStore('rows');
              const terminal = event => {
                trace.push(event.type, openRequest.transaction === tx);
                Promise.resolve().then(() => trace.push(openRequest.transaction === tx));
              };
              tx.oncomplete = tx.onabort = terminal;
              if (path === 'abort') tx.abort();
              if (path === 'constraint') { store.add('one', 1); store.add('duplicate', 1); }
            };
            openRequest.onsuccess = openRequest.onerror = event => {
              trace.push(event.type, openRequest.transaction === null);
              if (event.type === 'success') openRequest.result.close();
            };
            "#,
        );
        while doc.eval("trace.length") == "0" {
            let task = env.tasks.0.borrow_mut().pop_front().expect("expected an upgrade task");
            task();
        }
        let terminal = if path == "commit" { "complete" } else { "abort" };
        assert_eq!(doc.eval("trace.join('|')"), format!("{terminal}|true|true"), "{path}");
        assert_eq!(doc.eval("openRequest.transaction === null"), "true", "{path}: before open result task");
        env.tasks.drain();
        let outcome = if path == "commit" { "success" } else { "error" };
        assert_eq!(doc.eval("trace.join('|')"), format!("{terminal}|true|true|{outcome}|true"), "{path}");
    }
}

#[test]
fn upgrade_lifecycle_and_exception_order() {
    let env = Env::new();
    let d = env.doc("http://idb.test/");
    d.eval(HELPERS);
    let got = env.run(
        &d,
        r#"
        const log = [];
        const err = f => { try { f(); return 'ok'; } catch (e) { return e.name; } };
        const open = indexedDB.open('life', 1);
        open.onupgradeneeded = () => {
          const db = open.result, tx = open.transaction;
          log.push(open.readyState, tx.mode, tx.db === db, db.version);
          db.createObjectStore('s');
          tx.oncomplete = () => log.push('complete:' + err(() => db.createObjectStore('late')) + ':' + (open.transaction === tx));
        };
        const db = await req(open);
        log.push('success:' + open.transaction, err(() => db.createObjectStore('x')));
        db.close();
        log.push(err(() => db.transaction('s')));
        const again = indexedDB.open('life', 2);
        again.onupgradeneeded = () => {
          const tx = again.transaction, db2 = again.result;
          tx.abort();
          log.push('aborted:' + err(() => db2.createObjectStore('y')) + ':' + err(() => tx.abort()));
          tx.onabort = () => log.push('onabort:' + err(() => db2.createObjectStore('z')) + ':' + (again.transaction === tx) + ':' + db2.version);
        };
        log.push(await req(again).catch(e => e.name + ':' + again.transaction));
        // State checks precede argument checks; IDL conversions precede both.
        const db3 = await openDb('life');
        const store = db3.transaction('s').objectStore('s');
        await req(store.count());
        await new Promise(ok => db3.transaction('s').oncomplete = ok);
        log.push(err(() => store.get({})), err(() => store.getAll(1, -1)), err(() => store.openCursor(null, 'sideways')),
          err(() => store.put(1, 1)), err(() => db3.transaction('missing')), err(() => db3.transaction([])),
          err(() => db3.transaction('s', 'versionchange')), store.put.length, store.getAll.length);
        out = log.join('|');
    "#,
    );
    assert_eq!(
        got,
        "done|versionchange|true|1|complete:InvalidStateError:true|success:null|InvalidStateError|InvalidStateError|aborted:TransactionInactiveError:InvalidStateError|onabort:InvalidStateError:true:1|AbortError:null|TransactionInactiveError|TypeError|TypeError|TransactionInactiveError|NotFoundError|InvalidAccessError|TypeError|1|0"
    );
}

#[test]
fn crypto_keys_are_stored_and_read_back_as_usable_keys() {
    let env = Env::new();
    let a = env.doc("http://idb.test/keys");
    a.eval(HELPERS);
    let got = env.run(
        &a,
        r#"
        const key = await crypto.subtle.generateKey({name: 'AES-GCM', length: 256}, false, ['encrypt', 'decrypt']);
        const db = await openDb('keys', 1, (db) => db.createObjectStore('k'));
        await req(db.transaction('k', 'readwrite').objectStore('k').put({key}, 'main'));
        const back = (await req(db.transaction('k').objectStore('k').get('main'))).key;
        const iv = new Uint8Array(12);
        const sealed = await crypto.subtle.encrypt({name: 'AES-GCM', iv}, key, new Uint8Array([5]));
        const open = await crypto.subtle.decrypt({name: 'AES-GCM', iv}, back, sealed);
        out = [back instanceof CryptoKey, back !== key, back.extractable, back.algorithm.length, new Uint8Array(open)[0]].join();
        "#,
    );
    assert_eq!(got, "true,true,false,256,5");
}
