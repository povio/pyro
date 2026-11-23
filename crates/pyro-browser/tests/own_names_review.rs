//! Review tests for patch 0024 (perf/jsc-r2): computed-style names answered
//! as own properties, and indexed properties enumerable for every indexed
//! class. Every expected value below was taken from Chrome 157 (headless)
//! on the same expressions, except where a comment says it is
//! pyro's pre-existing behaviour.

use pyro_browser::*;
use pyro_js::Realm;

fn eval(realm: &Realm, source: &str) -> String {
    let scope = realm.scope().unwrap();
    scope
        .eval(source, Some("own-names-review.js"), 1)
        .unwrap_or_else(|e| panic!("{}: {}", source, e.details(&scope).text()))
        .to_string()
        .unwrap()
}

fn page() -> (BrowserThread, PageId, Realm) {
    let mut t = crate::browser_thread().unwrap();
    let page = t.browser.create_page().unwrap();
    let root = t.browser.page(page).unwrap().main_frame.main_realm().unwrap().clone();
    eval(
        &root,
        "document.body.innerHTML = `<style>.t{color:rgb(1,2,3);float:left}</style>
           <div id=a class='t x y'><b>1</b><b>2</b></div>
           <form id=f><input name=q value=1><input name=r></form>
           <select id=s><option>a</option><option>b</option></select>`; 0",
    );
    (t, page, root)
}

#[test]
fn computed_style_own_names_through_every_internal_method() {
    let (_t, _p, root) = page();
    let checks: &[(&str, &str)] = &[
        // HasProperty paths still reach the prototype.
        ("const cs = getComputedStyle(a); ['color' in cs, 'background-color' in cs, 'foo' in cs, Reflect.has(cs, 'color')].join()", "true,true,false,true"),
        ("Function('cs', 'with (cs) { return typeof color + \",\" + typeof cssFloat + \",\" + typeof foo; }')(getComputedStyle(a))", "string,string,undefined"),
        // [[GetOwnProperty]] paths.
        ("const d = Object.getOwnPropertyDescriptors(getComputedStyle(a)); [Object.keys(d).filter(k => isNaN(+k)).length, d.color.enumerable, d.color.value, 'background-color' in d, 'cssFloat' in d].join()", "741,true,rgb(1, 2, 3),false,false"),
        ("const e = Object.entries(getComputedStyle(a)); [e.filter(x => isNaN(+x[0])).length, e.find(x => x[0] === 'color')[1]].join()", "741,rgb(1, 2, 3)"),
        ("['color', 'background-color', 'cssFloat', 'epubWritingMode', 'length'].map(k => Object.prototype.propertyIsEnumerable.call(getComputedStyle(a), k)).join()", "true,true,false,false,false"),
        ("['color', 'Color', 'background-color', '-webkit-transform', 'webkitTransform', 'WebkitTransform', 'float', 'cssFloat', 'cssText', 'length', 'getPropertyValue', 'epubCaptionSide', 'foo', ''].map(k => getComputedStyle(a).hasOwnProperty(k) ? 1 : 0).join('')", "10111110000000"),
        ("[Object.hasOwn(getComputedStyle(a), 'color'), getComputedStyle(a).hasOwnProperty(Symbol.iterator), getComputedStyle(a).hasOwnProperty(Symbol.toStringTag)].join()", "true,false,false"),
        ("JSON.stringify(getComputedStyle(a), ['color', 'float', 'cssFloat'])", r#"{"color":"rgb(1, 2, 3)","float":"left","cssFloat":"left"}"#),
        // Proxies forward to the target's internal methods.
        ("const p = new Proxy(getComputedStyle(a), {}); [Object.prototype.hasOwnProperty.call(p, 'color'), Object.keys(p).filter(k => isNaN(+k)).length, 'color' in p, Object.getOwnPropertyDescriptor(p, 'color').value].join()", "true,741,true,rgb(1, 2, 3)"),
        ("Object.keys(new Proxy(getComputedStyle(a), { ownKeys: t => Reflect.ownKeys(t) })).filter(k => isNaN(+k)).length", "741"),
        // Integrity levels throw (the indices are not writable), as in Chrome.
        ("const out = []; for (const f of [Object.freeze, Object.seal]) { try { f(getComputedStyle(a)); out.push('ok'); } catch (e) { out.push(e.name); } } out.push(Object.isFrozen(getComputedStyle(a)), Object.isSealed(getComputedStyle(a))); out.join()", "TypeError,TypeError,false,false"),
        ("(() => { try { structuredClone(getComputedStyle(a)); return 'cloned'; } catch (e) { return e.name; } })()", "DataCloneError"),
        // An accessor defined on the object wins, and is enumerable.
        ("const c = getComputedStyle(a); Object.defineProperty(c, 'color', { get() { return 'G'; }, configurable: true }); const d = Object.getOwnPropertyDescriptor(c, 'color'); [c.color, typeof d.get, d.enumerable].join()", "G,function,true"),
        ("const out = []; for (const k in getComputedStyle(a)) out.push(k); const n = out.filter(k => isNaN(+k)); [n.length, n.includes('cssText'), n.indexOf('cssFloat') > n.indexOf('color')].join()", "750,true,true"),
    ];
    for (src, want) in checks {
        let body = match src.rfind("; ") { Some(i) if src.starts_with("const ") => format!("{}; return {}", &src[..i], &src[i + 2..]), _ => format!("return {src}") };
        assert_eq!(eval(&root, &format!("(() => {{ {body} }})()")), *want, "{src}");
    }
}

/// hasOwnProperty in a hot loop, then a read of the same name: both inline
/// caches (the read's, and hasOwnProperty's own cache) must keep giving the
/// right answer once the JITs compile the loop, across three wrappers of one
/// structure, a realm's iframe, and another realm's hasOwnProperty.
#[test]
fn computed_style_own_names_stay_right_in_compiled_loops() {
    let (_t, _p, root) = page();
    assert_eq!(
        eval(
            &root,
            "(() => { const c = getComputedStyle(a); let s = 0, h = 0;
               for (let i = 0; i < 20000; i++) { if (c.hasOwnProperty('color')) h++; s += c.color.length; }
               const cs = [c, getComputedStyle(document.body), getComputedStyle(document.createElement('i'))]; let t = 0;
               for (let i = 0; i < 30000; i++) { const x = cs[i % 3]; t += (Object.hasOwn(x, 'display') ? 1 : 0) + (x.hasOwnProperty('foo') ? 100 : 0) + x.display.length; }
               return [h, s, t].join(); })()"
        ),
        "20000,240000,130000"
    );
    assert_eq!(
        eval(
            &root,
            "(() => { const f = document.createElement('iframe'); document.body.append(f);
               const w = f.contentWindow; const c = w.getComputedStyle(w.document.body);
               return [c.hasOwnProperty('color'), Object.keys(c).filter(k => isNaN(+k)).length,
                       w.Object.prototype.hasOwnProperty.call(getComputedStyle(a), 'color'),
                       Object.getPrototypeOf(c) === CSSStyleDeclaration.prototype].join(); })()"
        ),
        "true,741,true,false"
    );
}

#[test]
fn indexed_properties_are_enumerable_for_every_indexed_class() {
    let (_t, _p, root) = page();
    let makers = [
        ("nodeList", "document.querySelectorAll('b')", "0,1"),
        ("childNodes", "a.childNodes", "0,1"),
        ("children", "a.children", "0,1"),
        ("byTag", "document.getElementsByTagName('b')", "0,1"),
        ("classList", "a.classList", "0,1,2"),
        ("attributes", "a.attributes", "0,1"),
        ("inlineStyle", "(a.style.color = 'red', a.style)", "0"),
        ("form", "f", "0,1"),
        ("formElements", "f.elements", "0,1"),
        ("select", "s", "0,1"),
        ("options", "s.options", "0,1"),
        ("styleSheets", "document.styleSheets", "0"),
    ];
    for (name, make, idx) in makers {
        let got = eval(
            &root,
            &format!(
                "(() => {{ const o = () => {make}; const ix = a => a.filter(k => !isNaN(+k)).join();
                   const d = Object.getOwnPropertyDescriptor(o(), 0);
                   const forin = []; for (const k in o()) forin.push(k);
                   return [d.enumerable, d.configurable, ix(Object.keys(o())), ix(Object.keys({{ ...o() }})),
                           ix(Object.entries(o()).map(e => e[0])), ix(Object.keys(Object.assign({{}}, o()))), ix(forin),
                           Object.prototype.propertyIsEnumerable.call(o(), '0')].join('|'); }})()"
            ),
        );
        assert_eq!(got, format!("true|true|{idx}|{idx}|{idx}|{idx}|{idx}|true"), "{name}");
    }
    // Strict writes to an index throw; sloppy ones are ignored.
    assert_eq!(
        eval(&root, "(() => { 'use strict'; const l = document.querySelectorAll('b'); try { l[0] = 1; return 'no throw'; } catch (e) { return e.name + ':' + (l[0] instanceof HTMLElement); } })()"),
        "TypeError:true"
    );
    assert_eq!(eval(&root, r#"JSON.stringify(document.querySelectorAll('b'))"#), r#"{"0":{},"1":{}}"#);
}

/// perf/jsc-r2-fixed: an own name's value is the property's own (the class's
/// value callback), not a `[[Get]]` through whatever prototype chain the
/// object has now; `hasOwnProperty` and `propertyIsEnumerable` need no value;
/// the listed names are the same on every call and in every realm.
#[test]
fn computed_style_own_values_and_names_after_prototype_changes() {
    let (_t, _p, root) = page();
    let checks: &[(&str, &str)] = &[
        // Chrome 157: true, "string:rgb(1, 2, 3)" (its [[Get]] also finds the
        // named property; ours goes to the prototype, which is gone).
        ("const c = getComputedStyle(a); Object.setPrototypeOf(c, null); const d = Object.getOwnPropertyDescriptor(c, 'color'); [Object.prototype.hasOwnProperty.call(c, 'color'), typeof d.value + ':' + d.value, d.enumerable].join()", "true,string:rgb(1, 2, 3),true"),
        // A read takes the new prototype's property (as in Chrome); the own
        // property's value stays the style's (Chrome: undefined).
        ("const c = getComputedStyle(a); Object.setPrototypeOf(c, { color: 'X' }); [c.color, Object.getOwnPropertyDescriptor(c, 'color').value, Object.hasOwn(c, 'color'), Object.prototype.propertyIsEnumerable.call(c, 'color')].join()", "X,rgb(1, 2, 3),true,true"),
        ("const c = getComputedStyle(a); Object.setPrototypeOf(c, null); [Object.entries(c).find(x => x[0] === 'float')[1], ({ ...c }).backgroundColor].join()", "left,rgba(0, 0, 0, 0)"),
        // The dashed and Webkit names answer with their property's value.
        ("const c = getComputedStyle(a); Object.setPrototypeOf(c, null); ['background-color', 'float', 'webkitTransform', 'WebkitTransform'].map(k => Object.getOwnPropertyDescriptor(c, k).value).join('|')", "rgba(0, 0, 0, 0)|left|none|none"),
        ("const c = getComputedStyle(a); const k1 = Object.getOwnPropertyNames(c).join(); const k2 = Object.getOwnPropertyNames(getComputedStyle(document.body)).join(); [k1 === k2, Object.keys(c).join() === Object.keys(c).join()].join()", "true,true"),
        ("const f = document.createElement('iframe'); document.body.append(f); const w = f.contentWindow; const k = w.Object.getOwnPropertyNames(w.getComputedStyle(w.document.documentElement)); [k.join() === Object.getOwnPropertyNames(getComputedStyle(a)).join(), k.length > 1000].join()", "true,true"),
        ("['color', 'epubCaptionSide', 'cssFloat', '0', 'length'].map(k => Object.prototype.propertyIsEnumerable.call(getComputedStyle(a), k)).join()", "true,false,false,true,false"),
    ];
    for (src, want) in checks {
        let body = match src.rfind("; ") { Some(i) if src.starts_with("const ") => format!("{}; return {}", &src[..i], &src[i + 2..]), _ => format!("return {src}") };
        assert_eq!(eval(&root, &format!("(() => {{ {body} }})()")), *want, "{src}");
    }
}
