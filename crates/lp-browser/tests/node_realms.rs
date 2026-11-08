//! The realm of a node across the documents of a frame tree: a node belongs to
//! the window of the document it was created in, whichever realm first asks for
//! it and whichever document it is adopted into. Also the realm a callback of
//! another window runs and reports in (NodeFilter, MutationObserver).

use lp_browser::*;
use lp_js::Realm;
use std::time::{Duration, Instant};

fn eval(realm: &Realm, source: &str) -> String {
    let scope = realm.scope().unwrap();
    scope
        .eval(source, Some("node-realms-test.js"), 1)
        .unwrap_or_else(|e| panic!("{}: {}", source, e.details(&scope).text()))
        .to_string()
        .unwrap()
}
fn page_with_frames() -> (BrowserThread, Realm) {
    let mut t = crate::browser_thread().unwrap();
    let page = t.browser.create_page().unwrap();
    let root = t.browser.page(page).unwrap().main_frame.main_realm().unwrap().clone();
    eval(
        &root,
        "globalThis.frame = () => { const f = document.createElement('iframe'); document.body.appendChild(f); return f; };
         globalThis.a = frame(); globalThis.b = frame(); globalThis.A = a.contentWindow; globalThis.B = b.contentWindow; 0",
    );
    (t, root)
}
fn settle(thread: &mut BrowserThread) {
    thread.run_until_idle(Some(Instant::now() + Duration::from_secs(3)));
}

#[test]
fn a_node_another_realm_reaches_first_is_wrapped_in_its_documents_realm() {
    let (_t, root) = page_with_frames();
    assert_eq!(
        eval(
            &root,
            "(() => {
               A.document.body.innerHTML = '<p>text</p>';
               const out = [];
               // A walker of this window over the frame's document.
               const walker = document.createTreeWalker(A.document, NodeFilter.SHOW_ELEMENT);
               walker.nextNode();
               out.push(walker.currentNode instanceof A.Node, walker.currentNode instanceof Node,
                        walker.currentNode === A.document.documentElement);
               // The node handed to a filter of this window, and a Text node.
               let seen;
               const it = document.createNodeIterator(A.document.body, NodeFilter.SHOW_TEXT, (n) => { seen = n; return 1; });
               const text = it.nextNode();
               out.push(seen === text, text instanceof A.Text, text instanceof Text, text === A.document.querySelector('p').firstChild);
               return out.join();
             })()"
        ),
        "true,false,true,true,true,false,true"
    );
}

#[test]
fn an_adopted_node_keeps_the_realm_of_the_document_it_was_created_in() {
    let (_t, root) = page_with_frames();
    assert_eq!(
        eval(
            &root,
            "(() => {
               const out = [];
               const container = A.document.createElement('div');
               container.innerHTML = '<p id=never-wrapped><b id=deep>x</b></p>';
               A.document.body.appendChild(container);
               // Through B, then into this document; reached through this document.
               B.document.body.appendChild(container);
               document.body.appendChild(container);
               const p = document.getElementById('never-wrapped'), deep = document.getElementById('deep');
               out.push(p.ownerDocument === document, p instanceof A.HTMLParagraphElement, p instanceof HTMLParagraphElement,
                        deep instanceof A.HTMLElement, p === container.firstChild, deep.firstChild instanceof A.Text);
               // A node parsed into the adopted container is created in this document.
               container.innerHTML = '<i></i>text';
               B.document.body.appendChild(container);
               const i = container.querySelector('i');
               out.push(container instanceof A.HTMLDivElement, i.ownerDocument === B.document, i instanceof HTMLElement,
                        i instanceof A.HTMLElement, i instanceof B.HTMLElement, container.lastChild instanceof Text);
               // A shadow root attached after adoption, and what is parsed into it.
               const host = A.document.createElement('div');
               document.body.appendChild(host);
               const shadow = host.attachShadow({ mode: 'open' });
               Object.getOwnPropertyDescriptor(A.ShadowRoot.prototype, 'innerHTML').set.call(shadow, '<p></p>');
               out.push(host instanceof A.HTMLElement, shadow.querySelector('p') instanceof HTMLParagraphElement);
               return out.join();
             })()"
        ),
        "true,true,false,true,true,true,true,true,true,false,false,true,true,true"
    );
}

#[test]
fn a_node_of_a_document_without_a_window_has_the_realm_of_the_script_that_created_the_document() {
    let (_t, root) = page_with_frames();
    assert_eq!(
        eval(
            &root,
            "(() => {
               const out = [];
               for (const doc of [A.document.implementation.createHTMLDocument(''),
                                  new A.DOMParser().parseFromString('<body></body>', 'text/html'),
                                  new A.Document()]) {
                 const container = doc.createElementNS('http://www.w3.org/1999/xhtml', 'div');
                 container.innerHTML = '<p></p>text<!--c-->';
                 document.body.appendChild(container);
                 const [p, text, comment] = document.body.lastChild.childNodes;
                 out.push(p.ownerDocument === document && p instanceof A.HTMLParagraphElement && text instanceof A.Text
                          && comment instanceof A.Comment && !(p instanceof HTMLParagraphElement));
               }
               // One made by this window: its nodes are this window's, also when the frame reaches them first.
               const mine = document.implementation.createHTMLDocument('');
               mine.body.innerHTML = '<p></p>';
               A.document.body.appendChild(mine.body.firstChild);
               const p = A.document.body.lastChild;
               out.push(p.ownerDocument === A.document, p instanceof HTMLParagraphElement, p instanceof A.HTMLParagraphElement);
               return out.join();
             })()"
        ),
        "true,true,true,true,true,false"
    );
}

#[test]
fn a_removed_frames_nodes_keep_one_wrapper_and_their_realm_until_its_realm_is_disposed() {
    let (mut t, root) = page_with_frames();
    assert_eq!(
        eval(
            &root,
            "(() => {
               const out = [];
               const P = A.HTMLParagraphElement, E = A.HTMLElement;
               const container = A.document.createElement('div');
               container.innerHTML = '<p id=p1><b id=b1>x</b></p>';
               A.document.body.appendChild(container);
               a.remove(); // without detaching the subtree first
               out.push(a.contentWindow === null);
               document.body.appendChild(container);
               // Through the container (a wrapper of the removed frame's realm)...
               const p = container.querySelector('p');
               out.push(p.ownerDocument === document, p instanceof P, document.body.lastChild === container);
               // ...and through this document.
               const b1 = document.getElementById('b1');
               out.push(b1 instanceof E, b1 instanceof HTMLElement, b1 === p.firstChild, b1.ownerDocument.defaultView === window);
               return out.join();
             })()"
        ),
        "true,true,true,true,true,false,true,true"
    );
    settle(&mut t);
    // The frame's realm is gone: a node never reached before is this window's.
    assert_eq!(eval(&root, "document.body.lastChild.firstChild.firstChild.firstChild instanceof Text"), "true");
}

#[test]
fn a_filter_of_another_window_runs_and_throws_in_its_own_realm() {
    let (_t, root) = page_with_frames();
    assert_eq!(
        eval(
            &root,
            "(() => {
               const out = [];
               const root = document.body;
               const thrown = (filter) => {
                 try { document.createTreeWalker(root, NodeFilter.SHOW_ELEMENT, filter).firstChild(); return 'no throw'; }
                 catch (e) { return e.constructor === A.TypeError ? 'A' : e.constructor === TypeError ? 'top' : String(e); }
               };
               const revoked = (W, target) => { const { proxy, revoke } = W.Proxy.revocable(target, {}); revoke(); return proxy; };
               const withMethod = new A.Object; withMethod.acceptNode = {};
               const withRevoked = new A.Object; withRevoked.acceptNode = revoked(window, () => {});
               out.push(thrown(new A.Object), thrown(withMethod), thrown(withRevoked), thrown(revoked(A, {})),
                        thrown(revoked(A, () => {})), thrown({}));
               // A filter of a removed frame is not called, and the traversal throws.
               const filter = new B.Function('window.called = true; return 1;');
               const it = document.createNodeIterator(root, NodeFilter.SHOW_ELEMENT, filter);
               out.push(it.nextNode() === root, B.called);
               B.called = false;
               const W = B;
               b.remove();
               try { it.nextNode(); out.push('no throw'); } catch (e) { out.push(e instanceof TypeError); }
               out.push(W.called, it.referenceNode === root);
               return out.join();
             })()"
        ),
        "A,A,A,A,A,top,true,true,true,false,true"
    );
}

#[test]
fn a_mutation_observer_callback_of_another_window_reports_its_exception_there() {
    let (mut t, root) = page_with_frames();
    eval(
        &root,
        "globalThis.calls = [];
         window.onerror = () => { calls.push('top'); };
         A.onerror = () => { calls.push('A'); };
         B.onerror = (message) => { calls.push('B:' + message); };
         const observer = new A.MutationObserver(new B.Function('throw new parent.Error(\"thrown\")'));
         observer.observe(A.document.body, { childList: true });
         A.document.body.append('x'); 0",
    );
    settle(&mut t);
    let calls = eval(&root, "calls.join()");
    assert!(calls.starts_with("B:") && calls.contains("thrown") && !calls.contains(','), "{calls}");
}
