//! NodeIterator / TreeWalker: the `NodeFilter` callback interface (Web IDL
//! "call a user object's operation"), the active flag, and the NodeIterator
//! pre-remove steps while a traversal is filtering.

use crate::support::network::Env;

fn env() -> Env {
    let env = Env::new("http://traversal.test/");
    env.eval(
        r#"globalThis.tree = (spec) => {
          // "root[a[a1],b[b1],c]" -> elements by name, root detached.
          const nodes = {}; let i = 0;
          const parse = (parent) => {
            let name = "";
            while (i < spec.length && /[a-z0-9]/.test(spec[i])) name += spec[i++];
            const el = nodes[name] = document.createElement(name + "-el");
            if (parent) parent.appendChild(el);
            if (spec[i] === "[") { i++; while (spec[i] !== "]") { parse(el); if (spec[i] === ",") i++; } i++; }
            return el;
          };
          parse(null);
          return nodes;
        }; 0"#,
    );
    env
}

#[test]
fn the_filter_is_an_object_whose_accept_node_is_read_on_every_call() {
    let env = env();
    assert_eq!(
        env.eval(
            r#"(() => {
              const n = tree("root[a,b]");
              const out = [];
              let reads = 0;
              const filter = { get acceptNode() { reads++; return () => NodeFilter.FILTER_ACCEPT; } };
              const w = document.createTreeWalker(n.root, NodeFilter.SHOW_ELEMENT, filter);
              out.push(w.filter === filter, w.firstChild() === n.a, w.nextSibling() === n.b, reads);
              // The method is looked up at call time, so one added later is used.
              const late = {};
              const w2 = document.createTreeWalker(n.root, NodeFilter.SHOW_ELEMENT, late);
              try { w2.firstChild(); out.push("no throw"); } catch (e) { out.push(e.constructor === TypeError); }
              late.acceptNode = (node) => node === n.b ? 1 : 3;
              out.push(w2.firstChild() === n.b);
              // `this` is the filter object; a function filter is called directly.
              let self;
              const w3 = document.createTreeWalker(n.root, 1, { acceptNode() { self = this; return 1; } });
              w3.firstChild();
              out.push(self === w3.filter);
              // A function with an `acceptNode` property is still called as a function.
              const fn = () => 2; fn.acceptNode = () => 1;
              out.push(document.createTreeWalker(n.root, 1, fn).firstChild());
              return out.join();
            })()"#
        ),
        "true,true,true,2,true,true,true,"
    );
}

#[test]
fn the_filter_result_is_an_unsigned_short_and_a_non_object_filter_is_a_type_error() {
    let env = env();
    assert_eq!(
        env.eval(
            r#"(() => {
              const n = tree("root[a]");
              const first = (f) => document.createTreeWalker(n.root, 1, f).firstChild();
              const out = [first(() => true) === n.a, first(() => 65537) === n.a, first(() => 65538), first(() => NaN),
                           first(() => "1") === n.a, first(() => -65535) === n.a];
              for (const bad of [5, "x", true]) {
                try { document.createTreeWalker(n.root, 1, bad); out.push("no throw"); }
                catch (e) { out.push(e instanceof TypeError); }
                try { document.createNodeIterator(n.root, 1, bad); out.push("no throw"); }
                catch (e) { out.push(e instanceof TypeError); }
              }
              out.push(document.createTreeWalker(n.root, 1, null).filter, document.createNodeIterator(n.root, 1, undefined).filter);
              return out.join();
            })()"#
        ),
        "true,true,,,true,true,true,true,true,true,true,true,,"
    );
}

#[test]
fn an_exception_of_the_filter_is_rethrown_as_is_and_clears_the_active_flag() {
    let env = env();
    assert_eq!(
        env.eval(
            r#"(() => {
              const n = tree("root[a]");
              const out = [];
              const thrown = { name: "test" };
              for (const filter of [
                () => { throw thrown; },
                { acceptNode() { throw thrown; } },
                { get acceptNode() { throw thrown; } },
                () => ({ valueOf() { throw thrown; } }),
              ]) {
                const w = document.createTreeWalker(n.root, 1, filter);
                const it = document.createNodeIterator(n.root, 1, filter);
                // Twice: the first failure must not leave the traversal "active".
                for (const run of [() => w.firstChild(), () => w.nextNode(), () => it.nextNode(), () => it.nextNode()]) {
                  try { run(); out.push("no throw"); } catch (e) { out.push(e === thrown); }
                }
                out.push(w.currentNode === n.root, it.referenceNode === n.root, it.pointerBeforeReferenceNode);
              }
              // A traversal started from inside the filter throws InvalidStateError.
              let inner;
              const w = document.createTreeWalker(n.root, 1, () => { try { w.nextNode(); } catch (e) { inner = e.name; } return 1; });
              out.push(w.firstChild() === n.a, inner);
              return out.join();
            })()"#
        ),
        "true,true,true,true,true,true,true,".repeat(4) + "true,InvalidStateError"
    );
}

#[test]
fn a_revoked_proxy_filter_throws_a_type_error() {
    let env = env();
    assert_eq!(
        env.eval(
            r#"(() => {
              const n = tree("root[a]");
              const out = [];
              const revoked = (target) => { const { proxy, revoke } = Proxy.revocable(target, {}); revoke(); return proxy; };
              for (const filter of [revoked({}), revoked(() => 1), { acceptNode: revoked(() => 1) }]) {
                const w = document.createTreeWalker(n.root, 1, filter);
                try { w.firstChild(); out.push("no throw"); } catch (e) { out.push(e.constructor === TypeError); }
              }
              // A live Proxy works, and its traps are not probed.
              let traps = 0;
              const live = new Proxy(() => 1, { get(t, k) { traps++; return t[k]; }, isExtensible(t) { traps++; return true; } });
              out.push(document.createTreeWalker(n.root, 1, live).firstChild() === n.a, traps);
              return out.join();
            })()"#
        ),
        "true,true,true,true,0"
    );
}

#[test]
fn removing_the_node_being_filtered_moves_the_position_of_the_traversal() {
    let env = env();
    assert_eq!(
        env.eval(
            r#"(() => {
              const out = [];
              // The accepted node removes itself: it is returned, the reference is the node before.
              let n = tree("root[a,b,c]");
              let it = document.createNodeIterator(n.root, 1, { acceptNode(node) { if (node === n.b) n.b.remove(); return 1; } });
              out.push(it.nextNode() === n.root, it.nextNode() === n.a, it.nextNode() === n.b,
                       it.referenceNode === n.a, it.pointerBeforeReferenceNode, it.nextNode() === n.c);
              // A rejected node removes itself: the traversal goes on from the node before it.
              n = tree("root[a,b,c]");
              it = document.createNodeIterator(n.root, 1, { acceptNode(node) { if (node === n.b) { n.b.remove(); return 2; } return 1; } });
              it.nextNode(); it.nextNode();
              out.push(it.nextNode() === n.c, it.referenceNode === n.c);
              // Backwards, pointer before the position, nothing after the removed subtree.
              n = tree("root[a[a1],b[b1]]");
              let armed = false;
              it = document.createNodeIterator(n.root, 1, { acceptNode(node) { if (armed && node === n.b1) n.b.remove(); return 1; } });
              for (let i = 0; i < 5; i++) it.nextNode();
              armed = true;
              out.push(it.previousNode() === n.b1, it.referenceNode === n.a1, it.pointerBeforeReferenceNode);
              // While filtering, the reference is still the last accepted node.
              n = tree("root[a,b]");
              const seen = [];
              it = document.createNodeIterator(n.root, 1, (node) => { seen.push(it.referenceNode.localName); return node === n.a ? 2 : 1; });
              it.nextNode(); it.nextNode();
              out.push(seen.join("|"));
              return out.join();
            })()"#
        ),
        "true,true,true,true,false,true,true,true,true,true,false,root-el|root-el|root-el"
    );
}

#[test]
fn removing_a_node_moves_the_reference_of_an_iterator() {
    let env = env();
    assert_eq!(
        env.eval(
            r#"(() => {
              const out = [];
              // An ancestor of the root is removed: nothing changes.
              let n = tree("root[a[a1],b[b1],c]");
              let it = document.createNodeIterator(n.a1);
              n.a.remove();
              out.push(it.referenceNode === n.a1, it.pointerBeforeReferenceNode);
              // Pointer before the reference, a following node inside the root.
              n = tree("root[a[a1],b[b1],c]");
              it = document.createNodeIterator(n.root);
              for (let i = 0; i < 5; i++) it.nextNode();
              it.previousNode();
              out.push(it.referenceNode === n.b1, it.pointerBeforeReferenceNode);
              n.b.remove();
              out.push(it.referenceNode === n.c, it.pointerBeforeReferenceNode);
              // No following node inside the root.
              n = tree("root[a[a1],b[b1]]");
              it = document.createNodeIterator(n.root);
              for (let i = 0; i < 5; i++) it.nextNode();
              it.previousNode();
              n.b.remove();
              out.push(it.referenceNode === n.a1, it.pointerBeforeReferenceNode);
              return out.join();
            })()"#
        ),
        "true,true,true,true,true,true,true,false"
    );
}
