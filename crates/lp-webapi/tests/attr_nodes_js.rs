//! `Attr` is a `Node`: accepted wherever a Node is, with the DOM Standard's
//! behaviour for a node that has no parent and no children. Plus the
//! attribute behaviours next to it: `createAttribute` names, attributes in
//! another namespace, `isEqualNode`, `DOMTokenList.replace`.

use crate::support::network::Env;

fn env() -> Env {
    Env::new("http://attr.test/")
}

/// Evaluates `body` (statements ending in `return`), mapping a thrown
/// exception to its name.
fn run(env: &Env, body: &str) -> String {
    env.eval(&format!("(() => {{ try {{ {body} }} catch (e) {{ return 'threw ' + e.name; }} }})()"))
}

const ATTR: &str = "const el = document.createElement('div'); el.setAttribute('x', 'abc'); const attr = el.getAttributeNode('x');";

#[test]
fn attr_is_a_node() {
    let env = env();
    assert_eq!(
        run(
            &env,
            &format!(
                "{ATTR} return [attr instanceof Node, attr instanceof EventTarget, Attr.prototype instanceof Node,
                  Object.getPrototypeOf(Attr.prototype) === Node.prototype, attr.nodeType, attr.nodeName,
                  attr.nodeValue, attr.textContent, attr.parentNode, attr.firstChild, attr.childNodes.length,
                  attr.hasChildNodes(), attr.isConnected, attr.ownerDocument === document,
                  attr.getRootNode() === attr, attr.ownerElement === el, attr.specified].join()"
            )
        ),
        "true,true,true,true,2,x,abc,abc,,,0,false,false,true,true,true,true"
    );
    // One object per attribute, from every accessor, until it leaves.
    assert_eq!(
        run(
            &env,
            &format!(
                "{ATTR} const same = attr === el.attributes[0] && attr === el.attributes.getNamedItem('x')
                    && attr === el.getAttributeNodeNS(null, 'x') && attr === el.attributes.x;
                 el.removeAttribute('x'); el.setAttribute('x', 'new');
                 return [same, attr.ownerElement, attr.value, el.getAttributeNode('x') === attr].join()"
            )
        ),
        "true,,abc,false"
    );
    // Lookups by qualified name lowercase it for an HTML element of an HTML
    // document only.
    assert_eq!(
        run(
            &env,
            "const xml = document.implementation.createDocument(null, '');
             const out = [];
             for (const el of [document.createElement('div'), document.createElementNS('', 'div'),
                               xml.createElementNS('http://www.w3.org/1999/xhtml', 'div')]) {
               el.setAttributeNS('foo', 'A:B', '1'); el.setAttributeNS('', 'j', '2');
               out.push([el.getAttributeNode('A:B'), el.attributes['A:B'], el.attributes.getNamedItem('A:B'),
                         el.getAttributeNode('J')].map(a => a ? a.name : 'none').join('/'));
             }
             return out.join()"
        ),
        "none/none/none/j,A:B/A:B/A:B/none,A:B/A:B/A:B/none"
    );
    // nodeValue / textContent / value all write the attribute.
    assert_eq!(
        run(
            &env,
            &format!(
                "{ATTR} attr.nodeValue = 'n'; const a = el.getAttribute('x'); attr.textContent = 't';
                 const b = el.getAttribute('x'); attr.value = 'v'; attr.nodeValue = null;
                 return [a, b, JSON.stringify(el.getAttribute('x'))].join()"
            )
        ),
        "n,t,\"\""
    );
    // Events can be dispatched at it.
    assert_eq!(
        run(&env, &format!("{ATTR} let n = 0; attr.addEventListener('x', () => n++); attr.dispatchEvent(new Event('x')); return n")),
        "1"
    );
}

#[test]
fn an_attr_is_never_a_child_or_a_parent() {
    let env = env();
    for call in [
        "p.appendChild(attr)",
        "p.insertBefore(attr, p.firstChild)",
        "p.replaceChild(attr, p.firstChild)",
        "p.append(attr)",
        "attr.appendChild(document.createElement('i'))",
    ] {
        assert_eq!(
            run(
                &env,
                &format!(
                    "const p = document.createElement('p'); p.appendChild(document.createElement('span'));
                     const attr = document.createAttribute('n'); {call}; return 'no throw'"
                )
            ),
            "threw HierarchyRequestError",
            "{call}"
        );
    }
    assert_eq!(
        run(
            &env,
            &format!(
                "{ATTR} el.setAttribute('y', '1'); const y = el.getAttributeNode('y');
                 return [attr.contains(attr), el.contains(attr), attr.isSameNode(attr), attr.isEqualNode(y),
                   attr.compareDocumentPosition(y), el.compareDocumentPosition(attr),
                   attr.lookupNamespaceURI(null), attr.isDefaultNamespace('http://www.w3.org/1999/xhtml')].join()"
            )
        ),
        "true,false,true,false,36,20,http://www.w3.org/1999/xhtml,true"
    );
}

#[test]
fn clone_import_and_adopt_an_attr() {
    let env = env();
    assert_eq!(
        run(
            &env,
            "const doc = document.implementation.createHTMLDocument('t');
             doc.body.setAttributeNS('http://example.com/', 'p:name', 'value');
             const original = doc.body.getAttributeNodeNS('http://example.com/', 'name');
             const imported = document.importNode(original, true);
             const clone = original.cloneNode();
             const out = [imported instanceof Attr, imported !== original, imported.prefix, imported.namespaceURI,
               imported.localName, imported.name, imported.value, imported.ownerElement,
               imported.ownerDocument === document, clone.ownerDocument === doc, clone.isEqualNode(original),
               original.ownerElement === doc.body];
             // adoptNode takes it off its element.
             document.adoptNode(original);
             out.push(original.ownerElement, original.ownerDocument === document, doc.body.attributes.length);
             return out.join()"
        ),
        "true,true,p,http://example.com/,name,p:name,value,,true,true,true,true,,true,0"
    );
}

#[test]
fn an_attr_moves_between_elements_and_documents() {
    let env = env();
    assert_eq!(
        run(
            &env,
            "const el = document.createElement('div'); el.setAttribute('x', 'first');
             const attr = el.attributes[0];
             const other = new Document(); const target = other.createElement('other');
             const out = [attr.ownerDocument === document];
             try { target.attributes.setNamedItem(attr); out.push('no throw'); } catch (e) { out.push(e.name); }
             el.removeAttribute('x');
             out.push(target.attributes.setNamedItem(attr), attr.ownerDocument === other, attr.ownerElement === target);
             // Replacing: the old Attr comes back, detached, with its value.
             const next = document.createAttribute('x'); next.value = 'second';
             const old = target.setAttributeNode(next);
             out.push(old === attr, old.ownerElement, old.value, target.getAttribute('x'), next.ownerDocument === other,
               target.attributes.length, target.setAttributeNode(next) === next);
             out.push(target.removeAttributeNode(next) === next, target.hasAttribute('x'));
             try { target.removeAttributeNode(next); } catch (e) { out.push(e.name); }
             return out.join()"
        ),
        "true,InUseAttributeError,,true,true,true,,first,second,true,1,true,true,false,NotFoundError"
    );
    // Replacing keeps the place in the list and takes the new prefix.
    assert_eq!(
        run(
            &env,
            "const el = document.createElement('div');
             el.setAttributeNS('urn:n', 'a:k', '1'); el.setAttribute('z', '2');
             const next = document.createAttributeNS('urn:n', 'b:k'); next.value = '3';
             el.setAttributeNodeNS(next);
             return [el.attributes[0] === next, el.attributes[0].name, el.getAttributeNS('urn:n', 'k'), el.attributes.length].join()"
        ),
        "true,b:k,3,2"
    );
}

#[test]
fn ranges_take_attr_boundary_points() {
    let env = env();
    let t = |body: &str| run(&env, &format!("{ATTR} const range = new Range(); {body}"));
    assert_eq!(
        t("range.setStart(attr, 0); range.setEnd(attr, 0);
           return [range.startContainer === attr, range.startOffset, range.endContainer === attr, range.collapsed,
             range.commonAncestorContainer === attr].join()"),
        "true,0,true,true,true"
    );
    // The length of an Attr is 0, whatever its value.
    assert_eq!(t("range.setStart(attr, 1); return 'no throw'"), "threw IndexSizeError");
    assert_eq!(t("range.setEnd(attr, 1); return 'no throw'"), "threw IndexSizeError");
    for method in ["setStartBefore", "setStartAfter", "setEndBefore", "setEndAfter", "selectNode"] {
        assert_eq!(t(&format!("range.{method}(attr); return 'no throw'")), "threw InvalidNodeTypeError", "{method}");
    }
    // Its root is itself: never the root of a document range.
    assert_eq!(t("return range.comparePoint(attr, 0)"), "threw WrongDocumentError");
    assert_eq!(t("return [range.isPointInRange(attr, 0), range.intersectsNode(attr)].join()"), "false,false");
    assert_eq!(
        t("range.selectNodeContents(attr); const other = new Range(); other.selectNodeContents(attr);
           const out = [range.comparePoint(attr, 0), range.isPointInRange(attr, 0), range.intersectsNode(attr),
             [0, 1, 2, 3].map(how => range.compareBoundaryPoints(how, other)).join('')];
           try { range.comparePoint(attr, 1); } catch (e) { out.push(e.name); }
           try { range.compareBoundaryPoints(Range.START_TO_START, new Range()); } catch (e) { out.push(e.name); }
           return out.join()"),
        "0,true,true,0000,IndexSizeError,WrongDocumentError"
    );
    assert_eq!(
        t("range.selectNodeContents(attr); range.deleteContents();
           const out = [range.extractContents().childNodes.length, range.cloneContents().childNodes.length,
             range.startContainer === attr, range.collapsed, JSON.stringify(range.toString()),
             range.cloneRange().startContainer === attr];
           try { range.insertNode(document.createElement('span')); } catch (e) { out.push(e.name); }
           try { range.surroundContents(document.createElement('span')); } catch (e) { out.push(e.name); }
           return out.join()"),
        "0,0,true,true,\"\",true,HierarchyRequestError,HierarchyRequestError"
    );
    assert_eq!(
        t("const c = document.createElement('div'); range.setStart(c, 0); range.insertNode(attr); return 'no throw'"),
        "threw HierarchyRequestError"
    );
    assert_eq!(
        t("new StaticRange({ startContainer: attr, startOffset: 0, endContainer: attr, endOffset: 0 }); return 'no throw'"),
        "threw InvalidNodeTypeError"
    );
}

#[test]
fn create_attribute_takes_the_whole_name_as_local_name() {
    let env = env();
    let describe = "(a) => [a.localName, a.name, a.prefix, a.namespaceURI, JSON.stringify(a.value), a.ownerElement].join('|')";
    for name in [":", "a:0", "0:a", ":a", "x:y:x", "title"] {
        assert_eq!(
            env.eval(&format!("({describe})(document.createAttribute({name:?}))")),
            format!("{name}|{name}|||\"\"|"),
            "{name}"
        );
    }
    // Lowercased in an HTML document only.
    assert_eq!(env.eval("document.createAttribute('TiTle:X').name"), "title:x");
    assert_eq!(
        env.eval("const xml = document.implementation.createDocument(null, null, null); const a = xml.createAttribute('TiTle:X'); a.name + '|' + a.localName + '|' + (a.ownerDocument === xml)"),
        "TiTle:X|TiTle:X|true"
    );
    for bad in ["", "a b", "a=b", "a>b", "a/b"] {
        assert_eq!(run(&env, &format!("document.createAttribute({bad:?}); return 'no throw'")), "threw InvalidCharacterError");
    }
    assert_eq!(
        env.eval(&format!("({describe})(document.createAttributeNS('urn:n', 'p:l'))")),
        "l|p:l|p|urn:n|\"\"|"
    );
}

#[test]
fn an_attribute_in_another_namespace_is_not_the_content_attribute() {
    let env = env();
    let ns = "https://example.com/attributes";
    // Inline style: the `style` attribute in no namespace, wherever it sits.
    for op in ["change", "replace", "remove", "append"] {
        assert_eq!(
            run(
                &env,
                &format!(
                    "const el = document.createElement('div'); const op = {op:?};
                     if (op !== 'append') el.setAttributeNS({ns:?}, 'style', 'before');
                     el.setAttributeNS(null, 'style', 'color: green');
                     if (op === 'append' || op === 'change') el.setAttributeNS({ns:?}, 'style', 'after');
                     else if (op === 'remove') el.removeAttributeNS({ns:?}, 'style');
                     else {{ const a = document.createAttributeNS({ns:?}, 'style'); a.value = 'after'; el.setAttributeNodeNS(a); }}
                     return el.getAttributeNS(null, 'style') + '|' + el.style.color"
                )
            ),
            "color: green|green",
            "{op}"
        );
    }
    // Reflected attributes read the one in no namespace; getAttribute the
    // first by qualified name.
    assert_eq!(
        run(
            &env,
            &format!(
                "const out = [];
                 const span = document.createElement('span'); span.setAttributeNS({ns:?}, 'slot', 'target');
                 out.push(JSON.stringify(span.slot), span.getAttribute('slot'), span.hasAttribute('slot'));
                 const details = document.createElement('details'); details.setAttributeNS({ns:?}, 'open', '');
                 out.push(details.open);
                 const frame = document.createElement('iframe'); frame.setAttributeNS({ns:?}, 'src', 'data:text/html,x');
                 out.push(JSON.stringify(frame.src));
                 const input = document.createElement('input'); input.setAttributeNS({ns:?}, 'type', 'file');
                 out.push(input.type);
                 return out.join()"
            )
        ),
        "\"\",target,true,false,\"\",text"
    );
    // A namespaced `slot` assigns no slot.
    assert_eq!(
        run(
            &env,
            &format!(
                "const host = document.createElement('div'); const slot = document.createElement('slot');
                 slot.name = 'target'; host.attachShadow({{ mode: 'open' }}).append(slot);
                 const child = document.createElement('span'); child.setAttributeNS({ns:?}, 'slot', 'target');
                 host.append(child);
                 const before = [child.assignedSlot, slot.assignedNodes().length];
                 child.setAttribute('id', 'c'); child.setAttributeNS(null, 'slot', 'target');
                 return before.concat(child.assignedSlot === slot).join()"
            )
        ),
        ",0,true"
    );
    // selectedIndex = -1 stays through namespaced attributes.
    assert_eq!(
        run(
            &env,
            &format!(
                "const select = document.createElement('select');
                 select.innerHTML = '<option selected>one</option><option>two</option>';
                 select.selectedIndex = -1; const out = [select.selectedIndex];
                 select.options[0].setAttributeNS({ns:?}, 'selected', '');
                 select.setAttributeNS({ns:?}, 'multiple', ''); select.setAttributeNS({ns:?}, 'size', '');
                 out.push(select.selectedIndex, select.value === '');
                 select.append(document.createElement('option')); out.push(select.selectedIndex);
                 return out.join()"
            )
        ),
        "-1,-1,true,0"
    );
}

#[test]
fn is_equal_node_follows_the_dom_standard() {
    let env = env();
    assert_eq!(
        run(
            &env,
            "const impl = document.implementation; const out = [];
             const dt = (n, p, s) => impl.createDocumentType(n, p, s);
             out.push(dt('a', 'p', 's').isEqualNode(dt('a', 'p', 's')), dt('a', 'p', 's').isEqualNode(dt('a', 'q', 's')),
               dt('a', 'p', 's').isEqualNode(dt('a', 'p', 't')), dt('a', 'p', 's').isEqualNode(dt('b', 'p', 's')));
             const el = (ns, q) => document.createElementNS(ns, q);
             out.push(el('urn:a', 'x').isEqualNode(el('urn:b', 'x')), el('urn:a', 'p:x').isEqualNode(el('urn:a', 'q:x')),
               el('urn:a', 'p:x').isEqualNode(el('urn:a', 'p:x')));
             const a = el('urn:a', 'x'), b = el('urn:a', 'x');
             a.setAttributeNS('urn:n', 'p:k', 'v'); a.setAttribute('o', '1');
             b.setAttribute('o', '1'); b.setAttributeNS('urn:n', 'q:k', 'v');
             out.push(a.isEqualNode(b));
             b.removeAttributeNS('urn:n', 'k'); b.setAttributeNS('urn:m', 'p:k', 'v');
             out.push(a.isEqualNode(b), a.isEqualNode(null), a.isEqualNode(a));
             const t = document.createElement('p'), u = document.createElement('p');
             t.innerHTML = 'a<b>c</b><!--d-->'; u.innerHTML = 'a<b>c</b><!--d-->';
             out.push(t.isEqualNode(u)); u.lastChild.data = 'e'; out.push(t.isEqualNode(u));
             return out.join()"
        ),
        "true,false,false,false,false,false,true,true,false,false,true,true,false"
    );
}

#[test]
fn doctype_names_and_create_document_arguments() {
    let env = env();
    for bad in ["edi:>", "edi:a ", "a\\tb", "a\\0b", " "] {
        assert_eq!(
            run(&env, &format!("document.implementation.createDocumentType('{bad}', '', ''); return 'no throw'")),
            "threw InvalidCharacterError",
            "{bad}"
        );
    }
    for ok in ["html", "edi:<", "1", "a=b", ""] {
        assert_eq!(
            run(&env, &format!("return document.implementation.createDocumentType('{ok}', 'p', 's').name")),
            ok
        );
    }
    assert_eq!(run(&env, "document.implementation.createDocument(); return 'no throw'"), "threw TypeError");
    assert_eq!(run(&env, "document.implementation.createDocument('urn:x'); return 'no throw'"), "threw TypeError");
    assert_eq!(run(&env, "return document.implementation.createDocument(null, null).childNodes.length"), "0");
}

#[test]
fn token_list_replace_keeps_the_first_of_either_token() {
    let env = env();
    let replace = |before: &str, old: &str, new: &str| {
        run(
            &env,
            &format!(
                "const el = document.createElement('div'); el.className = {before:?};
                 const r = el.classList.replace({old:?}, {new:?}); return r + '|' + el.className"
            ),
        )
    };
    assert_eq!(replace("c b a", "c", "a"), "true|a b");
    assert_eq!(replace("a b c", "c", "a"), "true|a b");
    assert_eq!(replace("a b c", "b", "d"), "true|a d c");
    assert_eq!(replace("a a a  b", "a", "a"), "true|a b");
    assert_eq!(replace("a b", "c", "a"), "false|a b");
    assert_eq!(replace("a b a", "b", "a"), "true|a");
}

#[test]
fn svg_a_has_a_rel_list() {
    let env = env();
    assert_eq!(
        run(
            &env,
            "const a = document.createElementNS('http://www.w3.org/2000/svg', 'a');
             a.setAttribute('rel', 'noopener next');
             const g = document.createElementNS('http://www.w3.org/2000/svg', 'g');
             return [Object.prototype.toString.call(a.relList), a.relList.length, a.relList.contains('next'), a.rel,
               a instanceof SVGAElement, a instanceof SVGGraphicsElement, g.relList].join()"
        ),
        "[object DOMTokenList],2,true,noopener next,true,true,"
    );
}
