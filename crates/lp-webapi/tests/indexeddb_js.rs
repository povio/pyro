//! IndexedDB on the real JavaScriptCore with a queued browser task loop:
//! requests, transactions (auto-commit, abort and rollback), upgrades and
//! version change across documents, keys, key paths, key generators, indexes
//! and cursors, and the context partition shared across navigations.
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use lp_dom::parse::parse_document;
use lp_dom::Dom;
use lp_js::{ClassDef, Engine, JsClass, Realm, RealmOptions};
use lp_webapi::*;

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
