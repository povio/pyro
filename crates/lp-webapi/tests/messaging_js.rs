//! Structured clone of Error, DOMException, Blob, File and FileList,
//! transfer lists (ArrayBuffer detaching, MessagePort transfer), the
//! cross-VM wire format of those types (BroadcastChannel uses it), and
//! same-window `postMessage` cloning and target origins.

use crate::support::network::Env;

fn env() -> Env {
    let env = Env::new("http://messaging.test/");
    env.realm.set_exception_reporter(Some(lp_webapi::lifecycle::report_exception));
    env
}

#[test]
fn structured_clone_copies_errors_and_dom_exceptions() {
    let env = env();
    assert_eq!(
        env.eval(
            r#"(() => {
              const e = new TypeError('m', {cause: 'c'}); e.foo = 1;
              const c = structuredClone(e), empty = structuredClone(new Error);
              const renamed = structuredClone(Object.assign(new Error('x'), {name: 'Custom'}));
              const cyc = new Error('cyc'); cyc.cause = cyc; const cc = structuredClone(cyc);
              const errors = [EvalError, RangeError, ReferenceError, SyntaxError, URIError]
                .map(C => structuredClone(new C('k')).constructor === C).join('/');
              const d = structuredClone(new DOMException('dm', 'NotFoundError'));
              const fake = Object.create(DOMException.prototype);
              let fakeError; try { structuredClone(fake); } catch (x) { fakeError = x.name; }
              return [c !== e, c.constructor === TypeError, c.message, c.cause, c.foo, Object.keys(c).length,
                c.stack === e.stack, empty.hasOwnProperty('message'), renamed.name, renamed instanceof Error,
                cc.cause === cc, errors, d instanceof DOMException, d.name, d.message, d.code, fakeError].join('|');
            })()"#
        ),
        "true|true|m|c||0|true|false|Error|true|true|true/true/true/true/true|true|NotFoundError|dm|8|DataCloneError"
    );
}

#[test]
fn structured_clone_copies_blobs_files_and_file_lists_sharing_bytes() {
    let env = env();
    env.eval(
        r#"globalThis.out = null; (async () => {
          const blob = new Blob(['abc'], {type: 'text/plain'});
          const file = new File(['xy'], 'n.txt', {type: 'text/x', lastModified: 42});
          const [b, f, again] = structuredClone([blob, file, blob]);
          const sub = structuredClone(new (class extends File {})([], 'z'));
          const input = document.createElement('input'); input.type = 'file';
          const list = structuredClone({l: input.files}).l;
          let response; try { structuredClone(new Response('')); } catch (e) { response = e.name; }
          out = [b !== blob, b instanceof Blob, b instanceof File, b === again, b.size, b.type, await b.text(),
            f instanceof File, f.name, f.lastModified, f.type, await f.text(),
            Object.getPrototypeOf(sub) === File.prototype, list instanceof FileList, list !== input.files, list.length,
            response].join('|');
        })()"#,
    );
    env.until("out !== null");
    assert_eq!(
        env.eval("out"),
        "true|true|false|true|3|text/plain|abc|true|n.txt|42|text/x|xy|true|true|true|0|DataCloneError"
    );
}

#[test]
fn structured_clone_transfers_array_buffers_and_message_ports() {
    let env = env();
    assert_eq!(
        env.eval(
            r#"(() => {
              const err = (f) => { try { f(); return 'ok'; } catch (e) { return e.name; } };
              const b = new Uint8Array([1, 2]).buffer;
              const c = structuredClone({b, v: new Uint8Array(b)}, {transfer: [b]});
              const r = new ArrayBuffer(4, {maxByteLength: 8}), rc = structuredClone(r, {transfer: [r]});
              const twice = new ArrayBuffer(1), keep = new ArrayBuffer(3), blob = new Blob();
              return [b.byteLength, c.b.byteLength, new Uint8Array(c.b)[1], c.v.buffer === c.b, r.byteLength,
                rc.resizable, rc.maxByteLength,
                err(() => structuredClone(b, {transfer: [b]})), err(() => structuredClone(1, {transfer: [twice, twice]})),
                twice.byteLength, err(() => structuredClone({keep, f() {}}, {transfer: [keep]})), keep.byteLength,
                err(() => structuredClone(blob, {transfer: [blob]})), err(() => structuredClone(1, {transfer: [1]})),
                err(() => structuredClone(1, {transfer: 1}))].join();
            })()"#
        ),
        "0,2,2,true,0,true,8,DataCloneError,DataCloneError,1,DataCloneError,3,DataCloneError,TypeError,TypeError"
    );
    // A transferred port keeps the entanglement and the queued messages; the
    // original is detached.
    env.eval(
        r#"globalThis.log = [];
          const {port1, port2} = new MessageChannel();
          port1.postMessage('queued');
          const moved = structuredClone({p: port2}, {transfer: [port2]}).p;
          moved.onmessage = (e) => { log.push('moved:' + e.data); moved.postMessage('reply'); };
          port1.onmessage = (e) => log.push('port1:' + e.data);
          port2.postMessage('from-detached');
          globalThis.check = [moved !== port2, moved instanceof MessagePort,
            (() => { try { structuredClone(port2, {transfer: [port2]}); return 'ok'; } catch (e) { return e.name; } })(),
            (() => { try { structuredClone(moved); return 'ok'; } catch (e) { return e.name; } })()].join();"#,
    );
    env.until("log.length >= 2");
    assert_eq!(
        env.eval("check + '|' + log.join()"),
        "true,true,DataCloneError,DataCloneError|moved:queued,port1:reply"
    );
}

#[test]
fn message_ports_travel_through_post_message_as_event_ports() {
    let env = env();
    env.eval(
        r#"globalThis.log = [];
          const a = new MessageChannel(), b = new MessageChannel();
          a.port2.onmessage = (e) => {
            log.push([e.ports.length, e.ports[0] instanceof MessagePort, e.data.p === e.ports[0], e.ports === e.ports].join(':'));
            e.ports[0].postMessage('via');
          };
          b.port2.onmessage = (e) => log.push('b:' + e.data);
          a.port1.postMessage({p: b.port1}, [b.port1]);
          const err = (f) => { try { f(); return 'ok'; } catch (e) { return e.name; } };
          const c = new MessageChannel(), held = new MessageChannel();
          globalThis.check = [err(() => a.port1.postMessage(1, [a.port1])), err(() => c.port1.postMessage(1, [c.port2])),
            new MessageEvent('message', {ports: [held.port1]}).ports[0] === held.port1,
            err(() => new MessageEvent('message', {ports: [1]}))].join();"#,
    );
    env.until("log.length >= 2");
    assert_eq!(env.eval("check + '|' + log.join()"), "DataCloneError,ok,true,TypeError|1:true:true:true,b:via");
}

#[test]
fn window_post_message_clones_checks_target_origin_and_transfers() {
    let env = env();
    env.eval(
        r#"globalThis.log = [];
          addEventListener('message', (e) => log.push([e.data.n, e.data.d instanceof Date, e.origin, e.source === window,
            e.ports.length, e.data.buf && e.data.buf.byteLength].join(':')));
          const o = {n: 1, d: new Date(0)}; postMessage(o, '*'); o.n = 9;
          postMessage({n: 2}, 'http://other.test');
          postMessage({n: 3}, {targetOrigin: 'http://messaging.test/some/path'});
          const buf = new ArrayBuffer(8), ch = new MessageChannel();
          postMessage({n: 4, buf}, '/', [buf, ch.port1]);
          postMessage({n: 5}, 'data:text/plain,opaque');
          const err = (f) => { try { f(); return 'ok'; } catch (e) { return e.name; } };
          globalThis.check = [buf.byteLength, err(() => postMessage(() => 1, '*')), err(() => postMessage(1, 'nope')),
            err(() => postMessage(1, {targetOrigin: 'http://foo bar'})), err(() => postMessage(1, {transfer: null})),
            err(() => postMessage())].join();"#,
    );
    env.until("log.length >= 3");
    assert_eq!(
        env.eval("check + '|' + log.join('|')"),
        "0,DataCloneError,SyntaxError,SyntaxError,TypeError,TypeError|\
         1:true:http://messaging.test:true:0:|3:false:http://messaging.test:true:0:|4:false:http://messaging.test:true:1:8"
    );
}

#[test]
fn wire_format_carries_errors_dom_exceptions_and_files() {
    let env = env();
    env.eval(
        r#"globalThis.out = null;
          const tx = new BroadcastChannel('wire'), rx = new BroadcastChannel('wire');
          rx.onmessage = async (e) => {
            const {err, dom, file, blob, list, again} = e.data;
            out = [err instanceof RangeError, err.message, err.cause, err.cause2, dom instanceof DOMException, dom.name,
              dom.message, file instanceof File, file.name, file.lastModified, await file.text(), blob instanceof File,
              blob.type, again === file, list instanceof FileList, list.length].join('|');
          };
          const file = new File(['q'], 'n', {lastModified: 5});
          const input = document.createElement('input'); input.type = 'file';
          tx.postMessage({err: new RangeError('r', {cause: 1}), dom: new DOMException('m', 'AbortError'), file,
            blob: new Blob([], {type: 'a/b'}), list: input.files, again: file});"#,
    );
    env.until("out !== null");
    assert_eq!(env.eval("out"), "true|r|1||true|AbortError|m|true|n|5|q|false|a/b|true|true|0");
}

#[test]
fn wire_format_carries_crypto_keys_but_not_untransferred_ports() {
    let env = env();
    env.eval(
        r#"globalThis.out = null;
          const tx = new BroadcastChannel('keys'), rx = new BroadcastChannel('keys');
          rx.onmessage = async (e) => {
            const {secret, pair, again} = e.data;
            const signed = await crypto.subtle.sign('HMAC', secret, new Uint8Array([1]));
            const ok = await crypto.subtle.verify('HMAC', original, signed, new Uint8Array([1]));
            const sig = await crypto.subtle.sign({name: 'ECDSA', hash: 'SHA-256'}, pair.privateKey, new Uint8Array([2]));
            const ec = await crypto.subtle.verify({name: 'ECDSA', hash: 'SHA-256'}, keys.publicKey, sig, new Uint8Array([2]));
            out = [secret instanceof CryptoKey, secret !== original, secret.extractable, secret.algorithm.hash.name,
              secret.algorithm.length, secret.usages.join('+'), again === secret, ok, pair.privateKey.type,
              pair.publicKey.algorithm.namedCurve, ec].join('|');
          };
          let failed = '';
          try { tx.postMessage(new MessageChannel().port1); } catch (e) { failed = e.name; }
          globalThis.cloneError = failed;
          (async () => {
            globalThis.original = await crypto.subtle.generateKey({name: 'HMAC', hash: 'SHA-384'}, false, ['sign', 'verify']);
            globalThis.keys = await crypto.subtle.generateKey({name: 'ECDSA', namedCurve: 'P-256'}, true, ['sign', 'verify']);
            tx.postMessage({secret: original, pair: keys, again: original});
          })();"#,
    );
    env.until("out !== null");
    assert_eq!(env.eval("cloneError"), "DataCloneError");
    assert_eq!(env.eval("out"), "true|true|false|SHA-384|1024|sign+verify|true|true|private|P-256|true");
}
