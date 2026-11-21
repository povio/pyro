//! `element.style` and `CSSStyleRule.style` over Stylo's declaration blocks:
//! validation, shorthand expansion, serialization and priorities.

use pyro_js::Realm;

fn eval(realm: &Realm, source: &str) -> String {
    let scope = realm.scope().unwrap();
    scope
        .eval(source, Some("css-declarations-test.js"), 1)
        .unwrap_or_else(|e| panic!("{}: {}", source, e.details(&scope).text()))
        .to_string()
        .unwrap()
}

fn page() -> (pyro_browser::BrowserThread, Realm) {
    let mut t = crate::browser_thread().unwrap();
    let page = t.browser.create_page().unwrap();
    let root = t.browser.page(page).unwrap().main_frame.main_realm().unwrap().clone();
    (t, root)
}

#[test]
fn shorthands_expand_into_their_longhands() {
    let (_t, root) = page();
    assert_eq!(
        eval(
            &root,
            "const el = document.createElement('div');
             el.style.margin = '1px 2px';
             [el.style.marginTop, el.style.marginRight, el.style.marginBottom, el.style.marginLeft, el.style.margin,
              el.style.length, el.style[0], el.getAttribute('style')].join('|')"
        ),
        "1px|2px|1px|2px|1px 2px|4|margin-top|margin: 1px 2px;"
    );
    // Removing a longhand breaks the shorthand up.
    assert_eq!(
        eval(
            &root,
            "const d = document.createElement('div');
             d.style.cssText = 'padding: 3px; color: blue';
             d.style.removeProperty('padding-left');
             [d.style.padding, d.style.paddingTop, d.style.cssText].join('|')"
        ),
        "|3px|padding-top: 3px; padding-right: 3px; padding-bottom: 3px; color: blue;"
    );
}

#[test]
fn invalid_values_are_ignored_and_values_serialize_as_in_chrome() {
    let (_t, root) = page();
    assert_eq!(
        eval(
            &root,
            "const el = document.createElement('div');
             el.style.width = '10px';
             el.style.width = '10';
             el.style.color = '#f00';
             el.style.setProperty('height', 'bogus');
             el.style.setProperty('opacity', '0.5', 'important');
             [el.style.width, el.style.color, el.style.height, el.style.getPropertyPriority('opacity'),
              el.style.getPropertyValue('opacity'), el.style.cssText].join('|')"
        ),
        "10px|rgb(255, 0, 0)||important|0.5|width: 10px; color: rgb(255, 0, 0); opacity: 0.5 !important;"
    );
}

#[test]
fn named_access_knows_real_properties_only() {
    let (_t, root) = page();
    assert_eq!(
        eval(
            &root,
            "const s = document.createElement('div').style;
             [ 'backgroundColor' in s, 'MozAppearance' in s, 'notAProperty' in s, typeof s.backgroundColor,
               s['background-color'] === '', s.webkitTransform === ''].join()"
        ),
        "true,false,false,string,true,true"
    );
    assert_eq!(
        eval(&root, "const style = document.createElement('div').style;
             style.MozAnimationDuration = '2s';
             [style.animationDuration, style.getPropertyValue('-moz-animation-duration'),
              'mozAnimationDuration' in style, 'AnimationDuration' in style].join('|')"),
        "2s|2s|false|false"
    );
    // A name that is no property is an ordinary expando.
    assert_eq!(
        eval(&root, "const e = document.createElement('div'); e.style.foo = 'x'; [e.style.foo, e.getAttribute('style')].join('|')"),
        "x|"
    );
    // Features the layout turns on (grid, columns) parse before any layout;
    // webkit-cased names are the webkit ones.
    assert_eq!(
        eval(
            &root,
            "const g = document.createElement('div');
             g.style.gridTemplateColumns = '1fr 2fr'; g.style.columnCount = '2'; g.style.WebkitTransform = 'none';
             [g.style.gridTemplateColumns, g.style.columnCount, g.style.webkitTransform, 'WebkitTransform' in g.style]
               .join('|')"
        ),
        "1fr 2fr|2|none|true"
    );
    // Properties Chrome has and the engine's Stylo does not parse are kept.
    assert_eq!(
        eval(
            &root,
            "const f = document.createElement('div');
             f.style.webkitLineClamp = '2'; f.style.width = '1px';
             [f.style.webkitLineClamp, f.style.getPropertyValue('-webkit-line-clamp'), f.style.length].join('|')"
        ),
        "2|2|2"
    );
}

#[test]
fn rule_styles_share_the_declaration_block_model() {
    let (_t, root) = page();
    assert_eq!(
        eval(
            &root,
            "const style = document.head.appendChild(document.createElement('style'));
             style.textContent = '.a { margin: 4px; width: 5 }';
             const rule = style.sheet.cssRules[0];
             const before = [rule.style.marginLeft, rule.style.width].join('/');
             rule.style.setProperty('color', 'lime');
             before + '|' + rule.style.cssText"
        ),
        "4px/|margin: 4px; color: lime;"
    );
}

#[test]
fn scroll_margin_reads_back_computed_and_inline() {
    // SloSki's club roster styles its row action with `[&_button]:scroll-mb-14` and reads
    // `getComputedStyle(button).scrollMarginBottom` back. The properties are Gecko-only in upstream Stylo, so
    // the declaration was dropped and every read was `""` (the camel-cased attribute and `getPropertyValue`
    // alike); they compute `56px` where the rule applies and the initial `0px` where it does not.
    let (_t, root) = page();
    assert_eq!(
        eval(
            &root,
            "(() => { const style = document.head.appendChild(document.createElement('style'));
             style.textContent = '.list button { scroll-margin-bottom: 3.5rem } \
                                  @media (max-width: 1px) { .list button.mobile { scroll-margin-bottom: 9px } } \
                                  .list button.root { scroll-margin-block-end: calc(1rem + 8px) }';
             const list = document.body.appendChild(document.createElement('div'));
             list.className = 'list';
             const button = list.appendChild(document.createElement('button'));
             const plain = document.body.appendChild(document.createElement('button'));
             const root = list.appendChild(document.createElement('button'));
             root.className = 'root';
             const cs = getComputedStyle(button), none = getComputedStyle(plain);
             return [cs.scrollMarginBottom, cs.getPropertyValue('scroll-margin-bottom'), cs['scroll-margin-bottom'],
              cs.scrollMarginTop, cs.scrollMarginInlineStart, cs.getPropertyValue('scroll-margin-block-end'),
              none.scrollMarginBottom, none.getPropertyValue('scroll-margin-bottom'),
              getComputedStyle(root).scrollMarginBottom,
              'scrollMarginBottom' in cs, [...cs].includes('scroll-margin-bottom')].join('|') })()"
        ),
        "56px|56px|56px|0px|0px|56px|0px|0px|24px|true|true"
    );
    // A restyle is seen; the shorthand and logical longhands compute onto their physical sides. A percentage and
    // a bogus value are invalid, so they change nothing: the inline physical side stays unset (a logical
    // declaration is not its alias) and the computed top is still the logical block-start's 7px.
    assert_eq!(
        eval(
            &root,
            "(() => { const el = document.body.appendChild(document.createElement('div'));
             const cs = getComputedStyle(el), out = [cs.scrollMarginTop];
             el.style.scrollMargin = '1px 2px 3px 4px';
             out.push([cs.scrollMarginTop, cs.scrollMarginRight, cs.scrollMarginBottom, cs.scrollMarginLeft].join(' '));
             out.push(el.style.scrollMarginLeft, el.style.cssText);
             el.style.cssText = 'scroll-margin-inline: 5px 6px; scroll-margin-block-start: 7px';
             out.push([cs.scrollMarginTop, cs.scrollMarginRight, cs.scrollMarginBottom, cs.scrollMarginLeft].join(' '));
             el.style.scrollMarginTop = '50%';
             out.push(el.style.scrollMarginTop);
             el.style.setProperty('scroll-margin-top', 'bogus');
             out.push(el.style.scrollMarginTop, cs.scrollMarginTop);
             return out.join('|') })()"
        ),
        "0px|1px 2px 3px 4px|4px|scroll-margin: 1px 2px 3px 4px;|7px 6px 0px 5px|||7px"
    );
}
