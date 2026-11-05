//! `document.styleSheets`, enumerable computed styles and resolved inset
//! values, the "request" guard of `Request.headers`, `<img srcset sizes>`
//! source selection, and the `hidden` / `writingSuggestions` attributes.

use lp_browser::*;
use lp_js::Realm;

fn eval(realm: &Realm, source: &str) -> String {
    let scope = realm.scope().unwrap();
    scope
        .eval(source, Some("cssom-and-attributes-test.js"), 1)
        .unwrap_or_else(|e| panic!("{}: {}", source, e.details(&scope).text()))
        .to_string()
        .unwrap()
}

fn page() -> (BrowserThread, Realm) {
    let mut t = crate::browser_thread().unwrap();
    let page = t.browser.create_page().unwrap();
    let root = t.browser.page(page).unwrap().main_frame.main_realm().unwrap().clone();
    (t, root)
}

#[test]
fn document_style_sheets_lists_style_elements_in_tree_order() {
    let (_t, root) = page();
    assert_eq!(
        eval(
            &root,
            "(() => { const a = document.head.appendChild(document.createElement('style'));
               a.textContent = '.a { color: red }';
               const b = document.body.appendChild(document.createElement('style'));
               const list = document.styleSheets;
               const out = [list === document.styleSheets, list.length, list[0] === a.sheet, list.item(1) === b.sheet,
                            list[2], list.item(2), list[0].cssRules[0].selectorText, [...list].length];
               a.remove();
               out.push(list.length, list[0] === b.sheet);
               return out.join(); })()"
        ),
        "true,2,true,true,,,.a,2,1,true"
    );
}

#[test]
fn style_rules_expose_their_declaration_block() {
    let (_t, root) = page();
    assert_eq!(
        eval(
            &root,
            "(() => { const el = document.body.appendChild(document.createElement('p'));
               const style = document.head.appendChild(document.createElement('style'));
               style.textContent = 'p { color: rgb(255, 0, 0); margin-top: 1px }';
               const rule = document.styleSheets[0].cssRules[0], out = [];
               out.push(rule.style === rule.style, rule.style.parentRule === rule, rule.style.length,
                        rule.style.color, rule.style.getPropertyValue('margin-top'));
               rule.style.color = 'rgb(0, 0, 255)';
               rule.style.setProperty('margin-top', '7px');
               out.push(getComputedStyle(el).color, getComputedStyle(el).marginTop, rule.cssText);
               rule.style = 'color: rgb(0, 128, 0)';
               out.push(getComputedStyle(el).color, rule.style.length, rule.style.removeProperty('color'), rule.style.cssText);
               return out.join('|'); })()"
        ),
        "true|true|2|rgb(255, 0, 0)|1px|rgb(0, 0, 255)|7px|p { color: rgb(0, 0, 255); margin-top: 7px; }|\
         rgb(0, 128, 0)|1|rgb(0, 128, 0)|"
    );
}

#[test]
fn computed_style_is_enumerable_and_reads_dashed_names() {
    let (_t, root) = page();
    assert_eq!(
        eval(
            &root,
            "(() => { const el = document.body.appendChild(document.createElement('div'));
               el.style.cssText = 'background-color: rgb(1, 2, 3); margin-top: 4px';
               const cs = getComputedStyle(el), names = [...cs];
               const detached = getComputedStyle(document.createElement('div'));
               return [cs.length > 100, cs.length === names.length, cs[0] === cs.item(0), names.includes('background-color'),
                       names.includes('margin-top'), names.includes('margin'), cs.item(100000) === '',
                       cs['background-color'], cs.backgroundColor, cs['margin-top'], cs['not-a-property'],
                       detached.length].join('|'); })()"
        ),
        "true|true|true|true|true|false|true|rgb(1, 2, 3)|rgb(1, 2, 3)|4px||0"
    );
}

#[test]
fn computed_insets_resolve_percentages_and_auto() {
    let (_t, root) = page();
    assert_eq!(
        eval(
            &root,
            "(() => { document.body.style.margin = '0';
               const box = document.body.appendChild(document.createElement('div'));
               box.style.cssText = 'position: relative; width: 200px; height: 100px; padding: 10px';
               const el = box.appendChild(document.createElement('div'));
               const read = (css) => { el.style.cssText = css; const cs = getComputedStyle(el);
                                       return [cs.top, cs.left, cs.bottom, cs.right].join(' '); };
               return [
                 read('position: relative; top: 10%; left: 25%'),
                 read('position: relative; top: calc(10% - 1px); right: 50%'),
                 read('position: sticky; top: 10%'),
                 read('position: absolute; width: 50px; height: 20px; top: 10%; left: 5px'),
                 read('position: absolute; width: 50px; height: 20px; bottom: 0; right: 0'),
                 read('position: static; top: 10%'),
               ].join(' / '); })()"
        ),
        "10px 50px -10px -50px / 9px -100px -9px 100px / 10px auto auto auto / \
         12px 5px 88px 165px / 100px 170px 0px 0px / 10% auto auto auto"
    );
}

#[test]
fn request_headers_drop_forbidden_headers_after_construction() {
    let (_t, root) = page();
    assert_eq!(
        eval(
            &root,
            "(() => { const r = new Request('https://site.example/'), h = r.headers;
               h.append('Cookie', 'a=b'); h.set('sec-x', '1'); h.append('X-Custom', 'ok');
               h.append('x-http-method-override', 'GET,track '); h.append('X-Method-Override', 'GETTRACE');
               const nc = new Request('https://site.example/', { mode: 'no-cors' }).headers;
               nc.append('X-Custom', 'no'); nc.append('Accept', 'text/html'); nc.set('content-type', 'application/json');
               const plain = new Headers(); plain.append('Cookie', 'a=b');
               return [h.has('cookie'), h.has('sec-x'), h.get('x-custom'), h.has('x-http-method-override'),
                       h.get('x-method-override'), nc.has('x-custom'), nc.get('accept'), nc.has('content-type'),
                       plain.get('cookie')].join(); })()"
        ),
        "false,false,ok,false,GETTRACE,false,text/html,false,a=b"
    );
}

#[test]
fn img_srcset_and_sizes_select_the_current_source() {
    let (_t, root) = page();
    assert_eq!(
        eval(
            &root,
            "(() => { const img = document.body.appendChild(document.createElement('img'));
               const name = () => img.currentSrc.split('/').pop();
               const out = [];
               img.src = 'fallback.png'; out.push(name());
               img.srcset = 'two.png 2x'; out.push(name());
               img.srcset = 'one.png 1x, two.png 2x'; out.push(name());
               img.srcset = 'small.png 100w, large.png 200w';
               img.sizes = '100px'; out.push(name());
               img.sizes = '200px'; out.push(name());
               img.sizes = '(max-width: 1px) 100px, calc(100px + 100px)'; out.push(name());
               img.sizes = 'bogus, 100px'; out.push(name());
               return out.join(); })()"
        ),
        "fallback.png,fallback.png,one.png,small.png,large.png,large.png,small.png"
    );
}

#[test]
fn hidden_until_found_and_writing_suggestions_reflect() {
    let (_t, root) = page();
    assert_eq!(
        eval(
            &root,
            "(() => { const outer = document.body.appendChild(document.createElement('div'));
               const inner = outer.appendChild(document.createElement('span'));
               const out = [inner.hidden, 'writingSuggestions' in inner, inner.writingSuggestions];
               inner.hidden = 'Until-Found'; out.push(inner.hidden, inner.getAttribute('hidden'));
               inner.hidden = 'yes'; out.push(inner.hidden, inner.getAttribute('hidden'));
               inner.hidden = 0; out.push(inner.hidden, inner.hasAttribute('hidden'));
               outer.setAttribute('writingsuggestions', 'FALSE'); out.push(inner.writingSuggestions);
               inner.writingSuggestions = 'true'; out.push(inner.writingSuggestions, inner.getAttribute('writingsuggestions'));
               inner.setAttribute('writingsuggestions', 'bogus'); out.push(inner.writingSuggestions);
               inner.setAttribute('writingsuggestions', ''); out.push(inner.writingSuggestions);
               return out.join(); })()"
        ),
        "false,true,true,until-found,until-found,true,,false,false,false,true,true,false,true"
    );
}
