//! `HTMLCollection`, `NodeList` and `NamedNodeMap` as WebIDL legacy platform
//! objects (expandos, refused writes, own property keys, iteration), the
//! qualified-name matching of `getElementsByTagName` and the table row and
//! cell operations.

use crate::support::network::Env;

fn env() -> Env {
    let env = Env::new("http://collections.test/");
    env.eval(
        r#"function kind(f) { try { f(); return "no throw"; } catch (e) { return e.constructor.name; } }
           function own(o) { return Object.getOwnPropertyNames(o).join(","); }
           function add(parent, tag, attrs, ns) {
             const el = ns === undefined ? document.createElement(tag) : document.createElementNS(ns, tag);
             for (const k in attrs) el.setAttribute(k, attrs[k]);
             return parent.appendChild(el);
           }
           "ready""#,
    );
    env
}

#[test]
fn an_expando_on_a_collection_shadows_a_named_property_added_later() {
    let env = env();
    assert_eq!(
        env.eval(
            r#"(() => {
              const c = document.getElementsByTagName("i");
              const out = [String(c.named)];
              c.named = "foo";
              const el = add(document.body, "i", { id: "named" });
              out.push(c.named, c.namedItem("named") === el, c[0] === el);
              delete c.named;
              out.push(c.named === el);
              // A non-configurable expando cannot be deleted.
              Object.defineProperty(c, "fixed", { value: 5 });
              add(document.body, "i", { id: "fixed" });
              out.push(c.fixed, delete c.fixed, kind(() => { "use strict"; delete c.fixed; }), c.fixed);
              return out.join("|");
            })()"#
        ),
        "undefined|foo|true|true|true|5|false|TypeError|5"
    );
}

#[test]
fn writes_to_indices_and_named_properties_of_a_collection_are_refused() {
    let env = env();
    assert_eq!(
        env.eval(
            r#"(() => {
              const c = document.getElementsByTagName("b");
              const el = add(document.body, "b", { id: "named" });
              const out = [];
              // Sloppy mode code: ignored.
              c[0] = "x"; c[3] = "y"; c.named = "z";
              out.push(c[0] === el, c[3], c.named === el, own(c));
              // Strict mode code: a TypeError.
              out.push(kind(() => { "use strict"; c[0] = "x"; }), kind(() => { "use strict"; c[3] = "x"; }),
                       kind(() => { "use strict"; c.named = "x"; }));
              out.push(c[0] === el, c[3], c.named === el, own(c));
              // The members stay live afterwards.
              add(document.body, "b", {}); add(document.body, "b", {}); const last = add(document.body, "b", { id: "last" });
              out.push(c.length, c[3] === last, own(c));
              // An expando over a prototype member is an ordinary property.
              c.item = "mine";
              out.push(c.item, typeof HTMLCollection.prototype.item);
              return out.join("|");
            })()"#
        ),
        "true||true|0,named|TypeError|TypeError|TypeError|true||true|0,named|4|true|0,1,2,3,named,last|mine|function"
    );
    // NodeList (indices only), NamedNodeMap and DOMTokenList follow the same rules.
    assert_eq!(
        env.eval(
            r#"(() => {
              const div = add(document.body, "div", { id: "host", class: "a b" });
              const text = div.appendChild(document.createTextNode("t"));
              const nodes = div.childNodes, attrs = div.attributes, tokens = div.classList;
              nodes[0] = 1; attrs[0] = 1; attrs.id = 1; tokens[0] = 1;
              return [nodes[0] === text, attrs[0].name, attrs.id.value, tokens[0],
                      kind(() => { "use strict"; nodes[0] = 1; }), kind(() => { "use strict"; nodes[9] = 1; }),
                      kind(() => { "use strict"; attrs[0] = 1; }), kind(() => { "use strict"; attrs.id = 1; }),
                      kind(() => { "use strict"; tokens[0] = 1; }),
                      kind(() => { "use strict"; attrs.other = 1; }), attrs.other,
                      nodes[0] === text, own(nodes), tokens[0], own(tokens)].join("|");
            })()"#
        ),
        "true|id|host|a|TypeError|TypeError|TypeError|TypeError|TypeError|no throw|1|true|0|a|0,1"
    );
}

#[test]
fn options_collection_indices_can_be_written() {
    let env = env();
    assert_eq!(
        env.eval(
            r#"(() => {
              "use strict";
              const select = add(document.body, "select", {});
              const o = (v, id) => { const el = document.createElement("option"); el.value = v; if (id) el.id = id; return el; };
              const values = () => Array.from(select.options, x => x.value || "-").join(",");
              const out = [];
              select.options[0] = o("a");
              select.options[select.options.length] = o("b");
              select.options[4] = o("e", "named");
              out.push(values());
              select.options[1] = o("B");
              out.push(values());
              select.options[2] = null; select.options[9] = null;
              out.push(values());
              out.push(kind(() => { select.options[0] = document.createElement("div"); }), kind(() => { select.options[0] = "x"; }));
              // Names: a visible one stays, another becomes an expando.
              select.options.named = 1; select.options.other = 2;
              out.push(select.options.named.value, select.options.other, select.options.length);
              select.options[4000000] = o("far");
              out.push(select.options.length);
              return out.join("|");
            })()"#
        ),
        "a,b,-,-,e|a,B,-,-,e|a,B,-,e|TypeError|TypeError|e|2|4|4"
    );
}

#[test]
fn supported_property_names_are_own_keys_but_not_enumerable() {

    let env = env();
    assert_eq!(
        env.eval(
            r#"(() => {
              const host = add(document.body, "section", {});
              add(host, "span", {});
              add(host, "span", { id: "" });
              add(host, "span", { id: "some-id" });
              add(host, "span", { id: "some-id" });
              add(host, "span", { name: "" });
              add(host, "span", { name: "some-name" });
              add(host, "span", { id: "another-id", name: "another-name" });
              // `name` only counts on HTML elements; an id that is a prototype
              // member's name or an array index is not a visible name.
              add(host, "span", { name: "foreign", id: "svg-id" }, "http://www.w3.org/2000/svg");
              add(host, "span", { id: "item", name: "0" });
              const c = host.children;
              const forIn = []; for (const k in c) if (c.hasOwnProperty(k)) forIn.push(k);
              const d = Object.getOwnPropertyDescriptor(c, "some-id");
              return [own(c), Reflect.ownKeys(c).join(","), Object.keys(c).join(","), forIn.join(","),
                      [d.value === c[2], d.writable, d.enumerable, d.configurable].join("/"),
                      c.hasOwnProperty("some-name"), "another-name" in c, c.hasOwnProperty("foreign")].join("|");
            })()"#
        ),
        "0,1,2,3,4,5,6,7,8,some-id,some-name,another-id,another-name,svg-id|\
         0,1,2,3,4,5,6,7,8,some-id,some-name,another-id,another-name,svg-id|\
         0,1,2,3,4,5,6,7,8|0,1,2,3,4,5,6,7,8|true/false/false/true|true|true|false"
    );
    // The keys follow the tree.
    assert_eq!(
        env.eval(
            r#"(() => {
              const host = add(document.body, "section", {});
              const c = host.children, out = [own(c)];
              const first = add(host, "p", { id: "div1" });
              add(host, "p", { id: "div2" });
              out.push(own(c));
              first.remove();
              out.push(own(c), Object.keys(c).join(","));
              c.expando = 1;
              out.push(own(c));
              return out.join("|");
            })()"#
        ),
        "|0,1,div1,div2|0,div2|0|0,expando,div2"
    );
    // NamedNodeMap: the attributes' qualified names; on an HTML element of an
    // HTML document not the ones with uppercase letters.
    assert_eq!(
        env.eval(
            r#"(() => {
              const el = add(document.body, "input", { id: "result", type: "text", value: "", width: "200px" });
              const out = [own(el.attributes), Object.keys(el.attributes).join(",")];
              el.removeAttribute("width");
              el.setAttributeNS("urn:x", "x:Upper", "1");
              el.setAttributeNS("urn:x", "x:lower", "1");
              out.push(own(el.attributes));
              const foreign = add(document.body, "g", {}, "http://www.w3.org/2000/svg");
              foreign.setAttributeNS(null, "viewBox", "0 0 1 1");
              out.push(own(foreign.attributes), foreign.attributes.viewBox.value);
              return out.join("|");
            })()"#
        ),
        "0,1,2,3,id,type,value,width|0,1,2,3|0,1,2,3,4,id,type,value,x:lower|0,viewBox|0 0 1 1"
    );
}

#[test]
fn only_declared_iterables_have_the_iterable_methods() {
    let env = env();
    assert_eq!(
        env.eval(
            r#"(() => {
              const div = add(document.body, "div", { class: "a b" });
              add(div, "p", { id: "1" }); add(div, "p", { id: "2" });
              const methods = o => ["entries", "keys", "values", "forEach"].filter(k => k in o).join(",");
              const ids = []; for (const p of div.children) ids.push(p.id);
              const names = []; for (const a of div.attributes) names.push(a.name);
              return [methods(div.children), methods(div.attributes), methods(div.childNodes), methods(div.classList),
                      Symbol.iterator in div.children, HTMLCollection.prototype[Symbol.iterator] === Array.prototype.values,
                      NamedNodeMap.prototype[Symbol.iterator] === Array.prototype.values, ids.join(","), names.join(","),
                      methods(document.createElement("select").options)].join("|");
            })()"#
        ),
        "||entries,keys,values,forEach|entries,keys,values,forEach|true|true|true|1,2|class|"
    );
}

#[test]
fn get_elements_by_tag_name_matches_qualified_names() {
    let env = env();
    let xhtml = "http://www.w3.org/1999/xhtml";
    // In an HTML document: HTML elements against the lowercased name, others exactly.
    assert_eq!(
        env.eval(&format!(
            r#"(() => {{
              const host = add(document.body, "div", {{}});
              const names = ["abc", "Abc", "ABC"];
              for (const ns of ["{xhtml}", "http://www.w3.org/2000/svg", "http://FOO"])
                for (const n of names) add(host, n, {{}}, ns).setAttribute("data-k", (ns === "{xhtml}" ? "h:" : ns.endsWith("svg") ? "s:" : "f:") + n);
              const found = q => Array.from(host.getElementsByTagName(q), e => e.getAttribute("data-k")).join(",");
              const prefixed = add(host, "te:ST", {{}}, "test");
              const colon = add(host, "a:b", {{}});
              return [found("abc"), found("Abc"), found("ABC"), found("*").split(",").length,
                      host.getElementsByTagName("te:ST")[0] === prefixed, host.getElementsByTagName("te:st").length,
                      host.getElementsByTagName("ST").length, host.getElementsByTagName("a:b")[0] === colon,
                      host.getElementsByTagName("A:B")[0] === colon, String(colon.prefix)].join("|");
            }})()"#
        )),
        "h:abc,s:abc,f:abc|h:abc,s:Abc,f:Abc|h:abc,s:ABC,f:ABC|11|true|0|0|true|true|null"
    );
    // In an XML document every element is matched exactly, and a collection
    // keeps the rule of the document its root was in when it was made.
    assert_eq!(
        env.eval(&format!(
            r#"(() => {{
              const parent = document.createElement("div");
              const kids = [["{xhtml}", "a"], ["{xhtml}", "A"], ["", "a"], ["", "A"]].map(([ns, n], i) => {{
                const el = parent.appendChild(document.createElementNS(ns, n)); el.setAttribute("k", i); return el; }});
              const keys = list => Array.from(list, e => e.getAttribute("k")).join("");
              const list = parent.getElementsByTagName("A");
              const out = [keys(list)];
              const xml = document.implementation.createDocument(null, "root", null);
              xml.documentElement.appendChild(parent);
              out.push(keys(list), keys(parent.getElementsByTagName("A")), keys(xml.getElementsByTagName("a")));
              for (const k of kids) parent.appendChild(k);
              out.push(keys(list), keys(parent.getElementsByTagName("A")));
              return out.join("|");
            }})()"#
        )),
        "03|03|13|02|03|13"
    );
}

#[test]
fn table_rows_sections_and_cells_can_be_inserted_and_deleted() {
    let env = env();
    assert_eq!(
        env.eval(
            r##"(() => {
              const table = add(document.body, "table", {});
              const shape = () => Array.from(table.children, s => s.localName + "(" + Array.from(s.children, r =>
                  r.localName + (r.id ? "#" + r.id : "") + (r.children.length ? "[" + r.children.length + "]" : "")).join(" ") + ")").join(" ");
              const out = [];
              // No rows and no tbody: a tbody is made for the row.
              table.insertRow().id = "a";
              table.insertRow(-1).id = "c";
              table.insertRow(1).id = "b";
              table.insertRow(0).id = "first";
              out.push(shape());
              const head = table.createTHead(), foot = table.createTFoot(), caption = table.createCaption();
              out.push(table.createTHead() === head, table.createTFoot() === foot, table.createCaption() === caption,
                       table.tHead === head, table.firstChild === caption, table.lastChild === foot);
              head.insertRow().id = "h";
              foot.insertRow(0).id = "f";
              const body2 = table.createTBody();
              out.push(body2.previousSibling === table.tBodies[0], shape());
              // table.rows: head rows, body rows, foot rows.
              out.push(Array.from(table.rows, r => r.id).join(","));
              const row = table.rows[1];
              row.insertCell(); row.insertCell(0).id = "x"; row.insertCell(-1); row.insertCell(1).id = "y";
              out.push(Array.from(row.cells, c => c.localName + c.id).join(","));
              row.deleteCell(0); row.deleteCell(-1);
              out.push(Array.from(row.cells, c => c.localName + c.id).join(","));
              table.deleteRow(1); table.deleteRow(-1);
              table.tBodies[0].deleteRow(0);
              out.push(Array.from(table.rows, r => r.id).join(","));
              table.deleteTHead(); table.deleteTFoot(); table.deleteCaption(); table.deleteCaption();
              out.push(shape());
              const names = ["IndexSizeError", "IndexSizeError", "IndexSizeError", "IndexSizeError", "TypeError"];
              const errors = [() => table.deleteRow(2), () => table.deleteRow(-2), () => table.insertRow(3),
                              () => table.rows[0].deleteCell(0), () => table.deleteRow()].map(f => {
                try { f(); return "no throw"; } catch (e) { return e.name; } });
              out.push(errors.join(",") === names.join(","));
              // -1 on an empty collection does nothing.
              table.rows[0].deleteCell(-1);
              return out.join("|");
            })()"##
        ),
        "tbody(tr#first tr#a tr#b tr#c)|true|true|true|true|true|true|true|\
         caption() thead(tr#h) tbody(tr#first tr#a tr#b tr#c) tbody() tfoot(tr#f)|h,first,a,b,c,f|tdx,tdy,td,td|tdy,td|\
         h,b,c|tbody(tr#b tr#c) tbody()|true"
    );
    // A live collection sees the deletion.
    assert_eq!(
        env.eval(
            r#"(() => {
              const table = add(document.body, "table", {});
              table.insertRow().insertCell().className = "text1";
              const c = table.getElementsByClassName("text1");
              const before = c.length;
              table.deleteRow(0);
              return before + "|" + c.length;
            })()"#
        ),
        "1|0"
    );
}
