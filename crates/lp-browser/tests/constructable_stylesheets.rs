//! Constructable stylesheets: `new CSSStyleSheet()`, `replace()` /
//! `replaceSync()`, and `adoptedStyleSheets` on documents and shadow roots
//! applied to computed styles.

use lp_js::Realm;

fn eval(realm: &Realm, source: &str) -> String {
    let scope = realm.scope().unwrap();
    scope
        .eval(source, Some("constructable-stylesheets-test.js"), 1)
        .unwrap_or_else(|e| panic!("{}: {}", source, e.details(&scope).text()))
        .to_string()
        .unwrap()
}

fn page() -> (lp_browser::BrowserThread, Realm) {
    let mut t = crate::browser_thread().unwrap();
    let page = t.browser.create_page().unwrap();
    let root = t.browser.page(page).unwrap().main_frame.main_realm().unwrap().clone();
    (t, root)
}

#[test]
fn adopted_sheets_style_the_document_and_shadow_trees() {
    let (_t, root) = page();
    assert_eq!(
        eval(
            &root,
            "(() => { const p = document.body.appendChild(document.createElement('p'));
               const cs = getComputedStyle(p), out = [];
               const sheet = new CSSStyleSheet();
               sheet.replaceSync('p { color: rgb(0, 128, 0) } @import url(x.css);');
               out.push(sheet.cssRules.length, sheet.ownerNode, document.adoptedStyleSheets.length, cs.color);
               document.adoptedStyleSheets = [sheet];
               out.push(cs.color, document.adoptedStyleSheets[0] === sheet,
                        document.adoptedStyleSheets === document.adoptedStyleSheets);
               sheet.insertRule('p { color: rgb(0, 0, 255) }', 1);
               out.push(cs.color);
               sheet.disabled = true;
               out.push(cs.color);
               sheet.disabled = false;
               document.adoptedStyleSheets = [];
               out.push(cs.color);
               const host = document.body.appendChild(document.createElement('div'));
               const shadow = host.attachShadow({ mode: 'open' });
               const span = shadow.appendChild(document.createElement('span'));
               const inner = new CSSStyleSheet({ media: 'screen, print' });
               inner.replaceSync('span { margin-top: 3px }');
               shadow.adoptedStyleSheets = [inner];
               out.push(getComputedStyle(span).marginTop, inner.media.length, inner.media.item(1), inner.media.mediaText);
               return out.join('|'); })()"
        ),
        "1||0|rgb(0, 0, 0)|rgb(0, 128, 0)|true|true|rgb(0, 0, 255)|rgb(0, 0, 0)|rgb(0, 0, 0)|3px|2|print|screen, print"
    );
}

#[test]
fn adopted_style_sheets_is_an_observable_array() {
    let (_t, root) = page();
    assert_eq!(
        eval(
            &root,
            "(() => { const p = document.body.appendChild(document.createElement('p'));
               const cs = getComputedStyle(p), out = [];
               const a = new CSSStyleSheet(), b = new CSSStyleSheet();
               a.replaceSync('p { color: rgb(0, 255, 0) !important }');
               b.replaceSync('p { color: rgb(0, 0, 255) !important }');
               const list = document.adoptedStyleSheets;
               list.push(a);
               out.push(cs.color);
               list.push(b);
               out.push(list.length, cs.color);
               list.pop();
               out.push(list.length, cs.color);
               list.push(b); list.reverse();
               out.push(cs.color);
               list.splice(1, 1);
               out.push(list.length, cs.color);
               const style = document.head.appendChild(document.createElement('style'));
               try { list.push(style.sheet); } catch (e) { out.push(e.name); }
               try { list.push('x'); } catch (e) { out.push(e.name); }
               try { document.adoptedStyleSheets = [style.sheet]; } catch (e) { out.push(e.name); }
               out.push(list.length);
               return out.join('|'); })()"
        ),
        "rgb(0, 255, 0)|2|rgb(0, 0, 255)|1|rgb(0, 255, 0)|rgb(0, 255, 0)|1|rgb(0, 0, 255)|NotAllowedError|TypeError|NotAllowedError|1"
    );
}

#[test]
fn replace_resolves_and_regular_sheets_refuse_it() {
    let (_t, root) = page();
    assert_eq!(
        eval(
            &root,
            "(() => { const out = [];
               const sheet = new CSSStyleSheet();
               const promise = sheet.replace('a { color: red }');
               out.push(promise instanceof Promise, sheet.cssRules.length);
               try { sheet.insertRule('@import url(x.css);'); } catch (e) { out.push(e.name); }
               const style = document.head.appendChild(document.createElement('style'));
               try { style.sheet.replaceSync('a {}'); } catch (e) { out.push(e.name); }
               out.push(typeof CSSStyleSheet, 'adoptedStyleSheets' in Document.prototype,
                        'adoptedStyleSheets' in ShadowRoot.prototype, 'replace' in CSSStyleSheet.prototype);
               return out.join('|'); })()"
        ),
        "true|1|SyntaxError|NotAllowedError|function|true|true|true"
    );
}

#[test]
fn one_sheet_shared_by_many_shadow_roots_styles_them_all() {
    let (_t, root) = page();
    assert_eq!(
        eval(
            &root,
            "(() => { const sheet = new CSSStyleSheet();
               sheet.replaceSync('b { padding-left: 4px }');
               const inner = [];
               for (let i = 0; i < 3; i++) {
                 const shadow = document.body.appendChild(document.createElement('div')).attachShadow({ mode: 'open' });
                 shadow.adoptedStyleSheets = [sheet, sheet];
                 inner.push(shadow.appendChild(document.createElement('b')));
               }
               const out = inner.map(b => getComputedStyle(b).paddingLeft);
               sheet.replaceSync('b { padding-left: 6px }');
               out.push(getComputedStyle(inner[2]).paddingLeft);
               return out.join(); })()"
        ),
        "4px,4px,4px,6px"
    );
}
