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

#[test]
fn attr_number_preserves_integer_tokens_and_taints_fallbacks() {
    let (_t, root) = page();
    let values = eval(&root, r#"
        document.body.innerHTML = `<style>p {
            z-index: attr(n number, 7);
            padding-left: attr(d em, 0px);
            font-size: 10px;
            background-image: attr(d number, url("data:,fallback"));
        }</style>
        <p id="integer" n=" /* prefix */ +1 /* suffix */ " d="1e1"></p>
        <p id="decimal" n="1.0"></p>
        <p id="invalid" n="1 2" d="invalid"></p>`;
        const styles = id => getComputedStyle(document.getElementById(id));
        [styles('integer').zIndex, styles('decimal').zIndex, styles('invalid').zIndex,
         styles('integer').paddingLeft, styles('decimal').backgroundImage, styles('invalid').backgroundImage].join('|')
    "#);
    assert_eq!(values, "1|auto|7|100px|none|none");
}

#[test]
fn inline_if_resolves_only_consulted_branches_and_predicates() {
    let (_t, root) = page();
    let actual = eval(&root, r#"
        document.body.innerHTML = `<style>
        @property --length { syntax: "<length>"; inherits: false; initial-value: 0px }
        @property --size { syntax: "<length>"; inherits: false; initial-value: 10px }
        body { --x: parent }
        p {
            --x: child;
            --length: 10px;
            --size: if(else: 13px; else: 1em);
            font-size: var(--size);
            --lazy: if(style(--x: child) and style(9px < --length < 11px): 37px; else: var(--lazy));
            --cycle: if(else: var(--cycle));
            --on: yes;
            --a: if(style(--on) or style(--b): 10px; else: 20px);
            --b: var(--a);
            left: var(--b);
            --self: if(style(--self): 1px; else: 29px);
            --negated: if(not style(--negated): 31px; else: 1px);
            --either: if(style(--either) or style(--x: child): 41px; else: 1px);
            padding: var(--self) var(--negated) var(--either);
            --empty: if(style(--missing): 97px);
            --fallthrough: if(unknown: bad; supports(display: block): 19px);
            margin-left: var(--lazy);
            margin-top: var(--cycle, 23px);
            margin-bottom: 11px var(--empty, 17px);
            margin-right: var(--fallthrough);
        }
        </style><p id="p">text</p>`;
        const cs = getComputedStyle(document.querySelector('#p'));
        ['left','font-size','padding-top','padding-right','padding-bottom','margin-left',
         'margin-top','margin-bottom','margin-right'].map(name => cs.getPropertyValue(name)).join('|')
    "#);
    assert_eq!(actual, "10px|13px|29px|31px|41px|37px|23px|11px|19px");
}

#[test]
fn inline_if_taints_only_urls_that_depend_on_attributes() {
    let (_t, root) = page();
    let actual = eval(&root, r#"
        document.body.innerHTML = `<style>
        p { --value: attr(data-value type(<number>)); --enabled: yes }
        #equality { background-image: if(style(--value: 7): url(data:image/gif;base64,R0lGODlhAQABAAAAACw=)) }
        #range { background-image: if(style(--value < 5): none; else: url(data:image/gif;base64,R0lGODlhAQABAAAAACw=)) }
        #failed {
            background-image: if(style(--enabled: var(--value) var(--missing)): none; else: url(data:image/gif;base64,R0lGODlhAQABAAAAACw=));
        }
        #failed-attr { background-image: if(style(--enabled: attr(n type(<number>))): none; else: url(data:,fallback)) }
        #nested {
            background-image: if(if(style(--value: 7): var(--missing)): none; else: url(data:image/gif;base64,R0lGODlhAQABAAAAACw=));
        }
        #clean { background-image: if(style(--enabled) or style(--value: 7): url(data:image/gif;base64,R0lGODlhAQABAAAAACw=)) }
        </style>
        <p id="equality" data-value="7"></p><p id="range" data-value="7"></p><p id="clean" data-value="7"></p><p id="failed" data-value="7"></p><p id="nested" data-value="7"></p><p id="failed-attr" n="invalid"></p>`;
        ['equality','range','failed','nested','failed-attr','clean']
            .map(id => getComputedStyle(document.getElementById(id)).backgroundImage).join('|')
    "#);
    assert_eq!(actual, r#"none|none|none|none|none|url("data:image/gif;base64,R0lGODlhAQABAAAAACw=")"#);
}

#[test]
fn font_affecting_properties_resolve_local_lengths_against_the_parent() {
    let (_t, root) = page();
    let computed = eval(&root, r#"
        document.documentElement.style.fontSize = '32px';
        document.body.innerHTML = `<style>
            main { font-size: 10px; line-height: 20px }
            #child {
                font-size: 50px;
                font-weight: calc(1em / 1px);
                font-style: oblique calc(1lh / 1px * 1deg);
                line-height: 2em;
                margin-left: 1em;
            }
            #grandchild { font-weight: calc(1rem / 1px) }
        </style><main><div id="child"><span id="grandchild"></span></div></main>`;
        const child = getComputedStyle(document.getElementById('child'));
        const grandchild = getComputedStyle(document.getElementById('grandchild'));
        [child.fontWeight, child.fontStyle, child.lineHeight, child.marginLeft, grandchild.fontWeight].join('|')
    "#);
    assert_eq!(computed, "10|oblique 20deg|100px|50px|32");
}

#[test]
fn a_local_url_retains_its_fragment_through_computed_style() {
    let (_t, root) = page();
    let actual = eval(&root, r##"
        document.head.innerHTML = '<base href="https://example.test/assets/">';
        document.body.innerHTML = '<div></div>';
        const el = document.body.firstChild;
        el.style.setProperty('--image', 'url("#mark")');
        el.style.backgroundImage = 'var(--image)';
        el.style.cursor = 'url(shape.svg#mark), auto';
        const computed = getComputedStyle(el);
        [computed.backgroundImage, computed.cursor].join('|')
    "##);
    assert_eq!(actual, r##"url("#mark")|url("https://example.test/assets/shape.svg#mark"), auto"##);
}

#[test]
fn inline_if_random_indexes_follow_selected_substitutions_in_order() {
    let (_t, root) = page();
    let actual = eval(&root, r#"
        const el = document.body.appendChild(document.createElement('div'));
        CSS.registerProperty({name:'--number',syntax:'<number>',initialValue:'1',inherits:false});
        el.style.setProperty('--indices', [
            'if(style(random(property-index-scoped, 0, 1) = random(ua-early---indices-1, 0, 1)): first; else: fail)',
            'if(else: if(style(random(property-index-scoped, 0px, 1px) = random(ua-early---indices-3, 0px, 1px)): nested; else: fail))',
            'if(style(--number: random(1,1)): registered; else: fail)',
            'if(style(random(property-index-scoped, 0, 1) = random(ua-early---indices-6, 0, 1)): last; else: fail)'
        ].join(' '));
        getComputedStyle(el).getPropertyValue('--indices')
    "#);
    assert_eq!(actual, "first nested registered last");
}

#[test]
fn extreme_insets_resolve_and_transition_in_layout_coordinates() {
    let (_t, root) = page();
    let values = eval(&root, r#"
        const target = document.body.appendChild(document.createElement('div'));
        target.style.cssText = 'position:absolute;left:33554400px';
        const value = () => getComputedStyle(target).left;
        const results = [value()];
        target.style.zoom = '2';
        target.style.left = 'calc(infinity * 1px)';
        results.push(value());
        target.style.zoom = '0.5';
        target.style.left = '-1e20px';
        results.push(value());
        target.style.zoom = '1';
        target.style.left = '0px';
        value();
        target.style.transition = 'left 100s steps(2, jump-end) -50s';
        target.style.left = 'calc(infinity * 1px)';
        results.push(value());
        results.join('|')
    "#);
    assert_eq!(values, "3.35544e+07px|1.67772e+07px|-6.71089e+07px|1.67772e+07px");
}

#[test]
fn typed_computed_numeric_retains_subpixel_precision_and_zoom() {
    let (_t, root) = page();
    assert_eq!(eval(&root, "document.body.style.order='16777217';document.body.computedStyleMap().get('order').value"), "16777217");
    assert_eq!(eval(&root, r#"
        document.body.innerHTML = `<style>
          #n { width: 99.99999px; zoom: 2; margin-left: 32.125%; height: auto }
        </style><div id="n"></div>`;
        const m = document.getElementById('n').computedStyleMap();
        const w = m.get('width'), p = m.get('margin-left');
        [w.value === Math.fround(99.99999), w.unit, p.value, p.unit, m.get('height').value].join('|')
    "#), "true|px|32.125|percent|auto");
}

#[test]
fn transitions_compare_bounded_absolute_inset_endpoints() {
    let (_t, root) = page();
    let compared = eval(&root, r#"
        const check = property => {
            const target = document.body.appendChild(document.createElement('div'));
            target.style.cssText = 'position:absolute;left:1e20px';
            const read = () => getComputedStyle(target).left;
            const transition = () => target.getAnimations().find(a => a.transitionProperty === 'left');
            read();
            target.style.transition = property + ' 100s steps(2, jump-end) -50s';
            target.style.left = '1e30px';
            read();
            const noConstantTransition = !transition();
            target.style.transition = 'none';
            target.style.left = '0px';
            read();
            target.style.transition = property + ' 100s steps(2, jump-end) -50s';
            target.style.left = '1e20px';
            const firstSample = read();
            const first = transition();
            target.style.left = '1e30px';
            const sameSample = read() === firstSample;
            const sameTransition = !!first && transition() === first;
            target.remove();
            return noConstantTransition && sameSample && sameTransition;
        };
        [check('left'), check('all')].join('|')
    "#);
    assert_eq!(compared, "true|true");
}

#[test]
fn palette_mix_weights_compute_in_the_elements_context() {
    let (_t, root) = page();
    let actual = eval(&root, r#"
        document.body.innerHTML = '<main><b></b><b id="palette"></b></main>';
        const el = document.querySelector('#palette');
        el.style.fontPalette = 'palette-mix(in lch, --a calc(10% * sibling-index()), --b)';
        const specified = el.style.fontPalette.includes('sibling-index()');
        const before = getComputedStyle(el).fontPalette;
        el.previousElementSibling.remove();
        const after = getComputedStyle(el).fontPalette;
        [specified, before, after].join('|')
    "#);
    assert_eq!(actual, "true|palette-mix(in lch, --a 20%, --b 80%)|palette-mix(in lch, --a 10%, --b 90%)");
}

#[test]
fn element_dependent_timing_uses_actual_attributes_and_pseudo_font_metrics() {
    let (_t, root) = page();
    let actual = eval(&root, r#"
        document.body.innerHTML = `<style>
            @keyframes step-test { to { opacity: .5 } }
            section { transition-delay: 5s }
            #target {
                font-size: 20px;
                animation-name: attr(data-name type(<custom-ident>), var(--missing));
                animation-duration: attr(data-time type(<time>), 3s);
                transition-delay: attr(missing, inherit);
            }
            @layer weak {
                #target#target { animation-name: wrong; animation-duration: 9s }
            }
            #target::before {
                content: "";
                font-size: 10px;
                transition-delay: calc(2s * sign(1em - 12px));
            }
        </style><section><i id="target" data-name="step-test" data-time="4s"></i></section>`;
        const target = document.getElementById('target');
        const computed = () => getComputedStyle(target);
        const animation = target.getAnimations().find(a => a.animationName === 'step-test');
        const result = [computed().animationName, computed().animationDuration, computed().transitionDelay,
            getComputedStyle(target, '::before').transitionDelay, animation?.effect.getTiming().duration];
        target.setAttribute('data-time', 'invalid');
        result.push(computed().animationDuration);
        target.setAttribute('data-name', '"invalid"');
        result.push(computed().animationName);
        result.join('|')
    "#);
    assert_eq!(actual, "step-test|4s|5s|-2s|4000|3s|none");
}

#[test]
fn menulist_line_height_is_adjusted_after_lh_units_are_computed() {
    let (_t, realm) = page();
    assert_eq!(eval(&realm, r#"
        document.body.innerHTML = '<div style="line-height:100px"><select multiple></select></div>';
        getComputedStyle(document.querySelector('select')).lineHeight
    "#), "normal");
    let result = eval(&realm, r#"
        document.body.innerHTML = '<style>select{line-height:100px;width:5lh}</style><select></select><select multiple></select><select style="appearance:none"></select><select style="appearance:menulist-button"></select>';
        [...document.querySelectorAll('select')].map(el => {
            const style = getComputedStyle(el);
            return style.width + '/' + style.lineHeight;
        }).join('|')
    "#);
    assert_eq!(result, "500px/normal|500px/100px|500px/100px|500px/normal");
}

#[test]
fn inherit_reads_the_parent_and_only_resolves_a_selected_fallback() {
    let (_thread, realm) = page();
    let values = eval(&realm, r#"
        document.head.innerHTML = `<style>
            @property --count { syntax: "<integer>"; inherits: false; initial-value: 2 }
            :root { --root-count: inherit(--count, 99) }
            #parent { --count: 7; --item: parent }
            #child {
                --name: --count; --count: 23;
                z-index: inherit(var(--name), 99);
                --item: inherit(--item, var(--item)) child;
                --absent: inherit(--missing);
                --empty: inherit(--missing,);
                --cyclic: inherit(--missing, var(--cyclic));
            }
        </style>`;
        document.body.innerHTML = '<div id="parent"><div id="child"></div></div>';
        const parentEl = document.getElementById('parent');
        const child = document.getElementById('child');
        child.style.setProperty('--checks', 'var(--absent, A)/var(--empty, B)/var(--cyclic, C)');
        const read = () => {
            const style = getComputedStyle(child);
            return [style.zIndex, style.getPropertyValue('--item')].join(':');
        };
        const before = read();
        parentEl.style.setProperty('--count', '11');
        parentEl.style.setProperty('--item', 'updated');
        [before, read(), getComputedStyle(child).getPropertyValue('--checks'),
         getComputedStyle(document.documentElement).getPropertyValue('--root-count')].join('|')
    "#);
    assert_eq!(values, "7:parent child|11:updated child|A//C|2");
}

#[test]
fn font_feature_value_maps_read_and_update_the_owning_rule() {
    let (_t, root) = page();
    let result = eval(&root, r#"
        const sheet = new CSSStyleSheet();
        sheet.replaceSync('@font-feature-values Demo { @swash { a: 4; a: sibling-index() } @styleset { b: 2 3; a: 1; b: 8 9 } }');
        const rule = sheet.cssRules[0], map = rule.styleset;
        const initial = [...map].map(([key, value]) => key + ':' + value.join(',')).join('/');
        const copy = map.get('b');
        copy[0] = 99;
        map.set('a', new Set([5, 7]));
        map.delete('b');
        map.set('b', 6);
        rule.fontFamily = 'Other Family';
        const visits = [];
        map.forEach((value, key, self) => visits.push(key + ':' + value.join(',') + ':' + (self === map)));
        let limits = '';
        try { rule.swash.set('bad', [1, 2]); } catch (e) { limits = e.name; }
        [initial, visits.join('/'), map === rule.styleset, rule.swash.get('a')[0],
         limits, rule.cssText.includes('@styleset { a: 5 7; b: 6; }'), rule.fontFamily].join('|')
    "#);
    assert_eq!(result, "b:8,9/a:1|a:5,7:true/b:6:true|true|4|InvalidAccessError|true|Other Family");
}

#[test]
fn custom_functions_resolve_typed_parameters_and_locals() {
    let (_t, root) = page();
    let actual = eval(&root, r#"
        const s = document.head.appendChild(document.createElement('style'));
        s.textContent = `
          @function --add(--x <length>, --y <length>: var(--x)) returns <length> {
            --copy: var(--x);
            result: calc(var(--copy) + var(--y));
          }
          @function --nested(--input) { result: --add(var(--input)); }
          #f { --input: 99px; width: --nested(9px); --out: --add(10px, 3px) }
        `;
        const el = document.body.appendChild(document.createElement('div'));
        el.id = 'f';
        const first = getComputedStyle(el).width;
        const out = getComputedStyle(el).getPropertyValue('--out');
        s.sheet.insertRule('@function --add(--x, --y: 0px) { result: 27px; }', s.sheet.cssRules.length);
        [first, out, getComputedStyle(el).width].join('|')
    "#);
    assert_eq!(actual, "18px|13px|27px");
}

#[test]
fn custom_functions_distinguish_typed_and_untyped_results() {
    let (_t, root) = page();
    let actual = eval(&root, r#"
        const s = document.head.appendChild(document.createElement('style'));
        s.textContent = `
          @function --typed() returns <number> { result: calc(2 + 3); }
          @function --tokens() { result: calc(2 + 3); }
          @function --shadow(--x) { --x: green; result: var(--x); }
          #f { --x: red; --a: --typed(); --b: --tokens(); color: --shadow(blue); }
        `;
        const el = document.body.appendChild(document.createElement('div')); el.id='f';
        const cs=getComputedStyle(el);
        [cs.getPropertyValue('--a'),cs.getPropertyValue('--b'),cs.color].join('|')
    "#);
    assert_eq!(actual, "5|calc(2 + 3)|rgb(0, 128, 0)");
}

#[test]
fn custom_functions_default_over_the_argument_scope_and_inherit_from_callers() {
    let (_t, root) = page();
    let actual = eval(&root, r#"
        const s = document.head.appendChild(document.createElement('style'));
        s.textContent = `
          @function --later(--a <length>: var(--b), --b <length>: 7px) { result: var(--a) }
          @function --optional(--a) { result: var(--a, omitted) }
          @function --unused(--a) { result: unused }
          @function --outer(--v) { --v: caller; result: --nested-inherit(); }
          @function --nested-inherit() { --v: hidden; result: inherit(--v, missing); }
          @function --wide() { --v: initial extra; result: var(--v); }
          #call { --v: element; --a: --later(); --b: --optional(); --c: --unused();
                  --d: --outer(); --e: --nested-inherit(); --f: --wide() }
        `;
        const el = document.body.appendChild(document.createElement('div')); el.id='call';
        const cs=getComputedStyle(el);
        ['--a','--b','--c','--d','--e','--f'].map(n=>cs.getPropertyValue(n)).join('|')
    "#);
    assert_eq!(actual, "7px|omitted|unused|caller|element|initial extra");
}

#[test]
fn custom_functions_test_local_style_values_in_their_own_scope() {
    let (_t, root) = page();
    let actual = eval(&root, r#"
        const s=document.head.appendChild(document.createElement('style'));
        s.textContent=`
          @function --test(--arg <length>) { --x: 3px;
            result: if(style(--arg: calc(1px + 2px)) and style(--x: 3px) and style(--x < 5px): yes; else: no) }
          #f { --x: 20px; --result: --test(3px) }
        `;
        const el=document.body.appendChild(document.createElement('div')); el.id='f';
        getComputedStyle(el).getPropertyValue('--result')
    "#);
    assert_eq!(actual, "yes");
}

#[test]
fn first_cssom_edit_of_a_rendered_style_updates_ordinary_rules() {
    let (_t, root) = page();
    let actual = eval(&root, r#"
        const s = document.head.appendChild(document.createElement('style'));
        s.textContent = '#first-edit { width: 11px; }';
        const el = document.body.appendChild(document.createElement('div'));
        el.id = 'first-edit';
        const width = () => getComputedStyle(el).width;
        const before = width();
        const index = s.sheet.insertRule('#first-edit { width: 23px; }', s.sheet.cssRules.length);
        const inserted = width();
        s.sheet.deleteRule(index);
        [before, inserted, width()].join('|')
    "#);
    assert_eq!(actual, "11px|23px|11px");
}

#[test]
fn function_slots_compute_against_the_calling_elements_font_metrics() {
    let (_t, root) = page();
    let actual = eval(&root, r#"
        document.body.innerHTML = `<style>
          @property --raw-length { syntax: "<length>"; inherits: false; initial-value: 1px }
          @function --twice(--x <length>: 1em) returns <length> {
            --local: var(--x); result: calc(2 * var(--local));
          }
          @function --line() returns <length> { result: 1lh }
          @function --tokens() { result: 1em }
          @function --constant(--unused <length>: 2em) returns <length> {
            --ignored-local: 3em; result: 23px;
          }
          section { font-size: 9px; line-height: 12px }
          #measure { font-size: 21px; line-height: 31px; --x: --twice(); --line: --line(); --raw-length: --tokens() }
          #ignore { --size: --constant(); font-size: var(--size) }
        </style><section><p id="measure"></p><p id="ignore"></p></section>`;
        const el = document.querySelector('#measure');
        const read = n => getComputedStyle(el).getPropertyValue(n);
        const before = [read('--x'), read('--line'), read('--raw-length')];
        el.style.fontSize = '33px';
        [...before, read('--x'), getComputedStyle(document.querySelector('#ignore')).fontSize].join('|')
    "#);
    assert_eq!(actual, "42px|31px|21px|66px|23px");
}

#[test]
fn style_rule_edits_retain_signed_zero_until_the_property_is_replaced() {
    let (_t, root) = page();
    let actual = eval(&root, r#"
        const el = document.body.appendChild(document.createElement('div'));
        el.id = 'signed-zero-rule';
        const sheet = document.head.appendChild(document.createElement('style')).sheet;
        sheet.insertRule('#signed-zero-rule { position: absolute; }');
        const rule = sheet.cssRules[0];
        const value = unit => 'clamp(-1, 1 / sign(sign(' + unit + ')), 1)';
        const z = () => getComputedStyle(el).zIndex;
        rule.style.zIndex = value('-0em');
        const first = z();
        const publicText = [rule.style.zIndex, rule.style.cssText, rule.cssText];
        rule.style.color = 'red';
        const groupIndex = sheet.insertRule('@media all { #signed-zero-rule { } }', sheet.cssRules.length);
        const group = sheet.cssRules[groupIndex];
        group.cssRules[0].style.order = value('-0em');
        const nested = getComputedStyle(el).order;
        const afterOtherEdits = z();
        rule.style.removeProperty('z-index');
        rule.style.setProperty('z-index', value('0em'));
        const positive = z();
        rule.style.cssText = 'position: absolute; z-index: ' + value('-0em') + ' !important';
        const priority = rule.style.getPropertyPriority('z-index');
        const replaced = z();
        const removed = rule.style.removeProperty('z-index');
        [first, publicText.every(s => s.includes('0em') && !s.includes('-0em')),
         afterOtherEdits, nested, !group.cssText.includes('-0em'), positive, priority, replaced,
         removed.includes('-0em'), z()].join('|')
    "#);
    assert_eq!(actual, "-1|true|-1|-1|true|1|important|-1|false|auto");
}

#[test]
fn constructed_style_rule_keeps_other_signed_zero_longhands_on_shorthand_edits() {
    let (_t, root) = page();
    let actual = eval(&root, r#"
        const el = document.body.appendChild(document.createElement('div'));
        el.id = 'signed-zero-margin';
        const sheet = new CSSStyleSheet();
        sheet.replaceSync('#signed-zero-margin { margin: 0; }');
        document.adoptedStyleSheets = [sheet];
        const rule = sheet.cssRules[0];
        rule.style.margin = '1px calc(1px * 10em / -0em) 2px';
        const negative = () => parseFloat(getComputedStyle(el).marginLeft) < -1e6;
        const before = negative();
        rule.style.marginRight = '4px';
        const afterSplit = negative();
        rule.style.removeProperty('margin-top');
        const afterRemove = negative();
        rule.style.cssText = rule.style.cssText;
        [before, afterSplit, afterRemove, parseFloat(getComputedStyle(el).marginLeft) > 1e6].join('|')
    "#);
    assert_eq!(actual, "true|true|true|true");
}

#[test]
fn inline_cssom_preserves_signed_zero_until_the_declarations_are_replaced() {
    let (_t, root) = page();
    let result = eval(&root, r#"
        const e = document.body.appendChild(document.createElement('p'));
        const num = n => `clamp(-1, 1 / sign(sign(${n}em)), 1)`;
        const read = el => getComputedStyle(el).zIndex;
        e.style.zIndex = num('-0');
        const out = [read(e), !e.getAttribute('style').includes('-0em'),
                     e.getAttribute('style') === e.style.cssText];
        e.style.color = 'red';
        e.style.zIndex = num('0');
        out.push(read(e), read(document.body.appendChild(e.cloneNode(true))));
        e.setAttribute('style', e.getAttribute('style'));
        out.push(read(e));
        e.style.cssText = e.style.cssText;
        out.push(read(e));
        e.style.removeProperty('z-index');
        e.style.cssText = 'z-index: ' + num('-0');
        out.push(read(e));
        e.setAttributeNS(null, 'style', 'z-index: ' + num('0'));
        out.push(read(e));
        e.style.cssText = 'z-index: ' + num('-0');
        e.removeAttribute('style');
        out.push(read(e), e.style.length);
        out.join('|')
    "#);
    assert_eq!(result, "-1|true|true|-1|-1|-1|1|-1|1|auto|0");
}

#[test]
fn inline_shorthand_updates_preserve_the_other_signed_longhands_and_priorities() {
    let (_t, root) = page();
    let result = eval(&root, r#"
        const e = document.body.appendChild(document.createElement('div'));
        const val = n => `calc(1px * clamp(-1, 1 / sign(sign(${n}em)), 1))`;
        e.style.setProperty('margin', val('-0') + ' ' + val('0'), 'important');
        const margins = () => {
            const cs = getComputedStyle(e);
            return [cs.marginTop, cs.marginRight, cs.marginBottom, cs.marginLeft].join(',');
        };
        const out = [margins(), !e.getAttribute('style').includes('-0em')];
        e.style.marginLeft = '6px';
        out.push(margins(), e.style.getPropertyPriority('margin-left'), e.style.getPropertyPriority('margin-top'));
        e.style.removeProperty('margin-top');
        out.push(margins());
        out.join('|')
    "#);
    assert_eq!(result, "-1px,1px,-1px,1px|true|-1px,1px,-1px,6px||important|0px,1px,-1px,6px");
}

#[test]
fn sibling_inline_styles_do_not_share_opposite_zero_signs_in_unresolved_math() {
    let (_thread, realm) = page();
    let result = eval(&realm, r#"
        document.body.innerHTML = '<div><i></i><i></i><i></i><i></i></div>';
        const kids = Array.from(document.querySelectorAll('i'));
        for (const [i, el] of kids.entries()) {
          const v = 'clamp(-1, 1 / sign(' + (i % 2 ? '-0em' : '0em') + '), 1)';
          if (i < 2) el.setAttribute('style', 'z-index:' + v);
          else el.style.zIndex = v;
        }
        kids.map(el => getComputedStyle(el).zIndex).join(',')
    "#);
    assert_eq!(result, "1,-1,1,-1");
}

#[test]
fn connected_keyframe_edits_retain_signed_zero_and_keyframe_grammar() {
    let (_t, root) = page();
    let result = eval(&root, r#"
        const s = document.head.appendChild(document.createElement('style'));
        const value = 'clamp(-1, 1 / sign(sign(-0em)), 1)';
        s.textContent = '@keyframes number-sign { from { z-index: '+value+'; } to { z-index: 7; } }';
        const anim = s.sheet.cssRules[0], from = anim.cssRules[0];
        const el = document.body.appendChild(document.createElement('div'));
        el.style.cssText = 'position:relative; z-index:13; animation:number-sign 100s step-end paused';
        const sample = () => getComputedStyle(el).zIndex;
        const before = sample();
        from.style.color = 'green';
        from.style.setProperty('z-index', '9', 'important');
        const afterSet = sample();
        from.style.removeProperty('color');
        const afterRemove = sample();
        from.style.cssText = 'z-index: '+value+'; opacity: 0.5 !important; animation-name: other;';
        const afterText = sample();
        const filtered = !from.style.opacity && !from.style.animationName;
        const publicText = [from.style.zIndex, from.style.cssText, from.cssText, anim.cssText]
            .every(text => text.includes('0em') && !text.includes('-0em'));
        const removed = from.style.removeProperty('z-index');
        [before, afterSet, afterRemove, afterText, filtered, publicText,
         removed.includes('0em') && !removed.includes('-0em'), sample()].join('|')
    "#);
    assert_eq!(result, "-1|-1|-1|-1|true|true|true|13");
}

#[test]
fn margin_rule_keeps_signed_zero_private_during_cssom_edits() {
    let (_t, realm) = page();
    let result = eval(&realm, r#"
        const sheet = new CSSStyleSheet();
        sheet.replaceSync('@page { @top-left { width:calc(1px * sign(-0em)); } }');
        const pageRule = sheet.cssRules[0], margin = pageRule.cssRules[0];
        margin.style.color = 'red';
        const serializations = () => [margin.style.width, margin.style.cssText, margin.cssText, pageRule.cssText];
        const clean = () => serializations().every(t => t.includes('0em') && !t.includes('-0em'));
        const set = clean();
        margin.style.removeProperty('color');
        [set, clean()].join('|')
    "#);
    assert_eq!(result, "true|true");
}

#[test]
fn cached_longhand_writes_follow_external_edits_ranges_and_priority() {
    let (_t, root) = page();
    let actual = eval(&root, r#"
        const el = document.body.appendChild(document.createElement('div'));
        const style = el.style, z = 'clamp(-1, 1 / sign(-0em), 1)';
        style.cssText = `opacity: .2; --label: "a;b"; z-index: ${z}; color: red; letter-spacing: 1px`;
        style.opacity = '0.75';
        style.letterSpacing = '120px';
        style.color = 'rebeccapurple';
        style.opacity = '1';
        style.letterSpacing = '3px';
        style.setProperty('color', 'blue', 'important');
        const read = () => getComputedStyle(el);
        const out = [read().zIndex, style.opacity, style.letterSpacing, style.getPropertyPriority('color'),
                     style.getPropertyValue('--label'), !el.getAttribute('style').includes('-0em')];
        el.setAttribute('style', `color: green; z-index: ${z}; margin-left: 4px; margin-inline-start: 9px; opacity: .25`);
        style.opacity = '0.5';
        style.marginLeft = '7px';
        out.push(read().color, read().marginLeft, read().zIndex, style.letterSpacing,
                 style.getPropertyPriority('color'), style.opacity);
        style.margin = '2px 4px';
        style.marginLeft = '11px';
        out.push(read().marginLeft, read().marginRight, read().zIndex);
        out.join('|')
    "#);
    assert_eq!(actual, r#"-1|1|3px|important|"a;b"|true|rgb(0, 128, 0)|7px|-1|||0.5|11px|4px|-1"#);
}

#[test]
fn assigned_nodes_count_all_light_tree_siblings_across_slot_changes() {
    let (_t, realm) = page();
    let result = eval(&realm, r#"
        const host = document.body.appendChild(document.createElement('section'));
        host.innerHTML = '<i slot="unused"></i><b slot="named"></b><i id="target"></i><b slot="unused"></b>';
        const target = host.querySelector('#target');
        const root = host.attachShadow({mode:'open'});
        root.innerHTML = '<style>slot::slotted(*) { z-index:sibling-index(); order:sibling-count() }</style><slot name="named"></slot><slot></slot>';
        const position = () => { const s=getComputedStyle(target); return s.zIndex + '/' + s.order; };
        const observed=[position()];
        const unassigned = document.createElement('b'); unassigned.slot='unused'; host.prepend(unassigned);
        observed.push(position());
        target.slot='named';
        observed.push(position());
        host.firstElementChild.remove();
        observed.push(position());
        observed.join('|')
    "#);
    assert_eq!(result, "3/4|4/5|4/5|3/4");
}

#[test]
fn dynamic_range_limit_mixes_recompute_in_sheets_and_inherit_on_sdr() {
    let (_t, root) = page();
    let actual = eval(&root, r#"
        const name = 'dynamic-range-limit';
        const mix = (...entries) => 'dynamic-range-limit-mix(' + entries.join(', ') + ')';
        const sheet = new CSSStyleSheet();
        sheet.replaceSync('#host { font-size: 20px }');
        document.adoptedStyleSheets = [sheet];
        document.body.innerHTML = '<div id="host"><span></span></div>';
        const host = document.querySelector('#host'), child = host.firstChild;
        const rule = sheet.cssRules[0].style;
        const read = el => getComputedStyle(el).getPropertyValue(name);
        const out = [read(child), matchMedia('(dynamic-range: standard)').matches,
                     matchMedia('(dynamic-range: high)').matches];
        rule.setProperty(name, mix('no-limit 20%', mix('standard calc(25% * sign(1em - 15px))', 'constrained 25%') + ' 80%'));
        out.push(read(host), read(child));
        host.style.fontSize = '15px';
        out.push(read(child));
        child.style.dynamicRangeLimit = 'standard';
        child.style.dynamicRangeLimit = mix('standard 101%', 'constrained 1%');
        out.push(child.style.getPropertyValue(name), read(child));
        child.style.removeProperty(name);
        rule.setProperty(name, 'no-limit');
        out.push(read(child));
        out.join('|')
    "#);
    assert_eq!(actual, concat!(
        "no-limit|true|false|dynamic-range-limit-mix(standard 40%, constrained 40%, no-limit 20%)|",
        "dynamic-range-limit-mix(standard 40%, constrained 40%, no-limit 20%)|",
        "dynamic-range-limit-mix(constrained 80%, no-limit 20%)|standard|standard|no-limit"
    ));
}

#[test]
fn dynamic_range_limit_accepts_either_term_order_and_zero_weight_mixes() {
    let (_t, root) = page();
    let actual = eval(&root, r#"
        const name = 'dynamic-range-limit';
        const values = [
            'dynamic-range-limit-mix(20% no-limit, 80% standard)',
            'dynamic-range-limit-mix(5% constrained)',
            'dynamic-range-limit-mix(standard 0%, constrained 0%)'
        ];
        const el = document.body.appendChild(document.createElement('i'));
        document.body.style.setProperty(name, 'standard');
        const out = [];
        for (const value of values) {
            el.style.setProperty(name, value);
            out.push(CSS.supports(name, value), getComputedStyle(el).getPropertyValue(name));
        }
        out.join('|')
    "#);
    // CSS Color HDR §§3.2–3.3: a zero-weight mix computes to the initial value,
    // including when the element would inherit another limit.
    assert_eq!(actual, "true|dynamic-range-limit-mix(standard 80%, no-limit 20%)|true|constrained|true|no-limit");
}

#[test]
fn typed_maps_reify_shorthands_background_layers_and_column_rule_values() {
    let (_t, root) = page();
    let actual = eval(&root, r#"
        const el = document.body.appendChild(document.createElement('div'));
        const declared = el.attributeStyleMap, computed = el.computedStyleMap();
        el.style.cssText = 'margin: 1px; background-size: 21px auto, cover; column-rule: 2px solid red;';
        const type = v => v.constructor.name;
        const out = [type(declared.get('margin')), type(computed.getAll('margin')[0]),
                     declared.getAll('background-size').map(type).join(','),
                     declared.get('background-size').value, type(declared.get('column-rule-width'))];
        el.style.backgroundSize = '21px auto, auto 3px';
        out.push(declared.getAll('background-size').length, type(declared.get('background-size')));
        declared.set('column-rule-style', new CSSKeywordValue('dotted'));
        declared.set('column-rule-width', CSS.px(7));
        out.push(type(declared.get('column-rule-style')), type(computed.get('column-rule-style')),
                 declared.getAll('column-rule-width')[0].value, type(computed.get('column-rule-width')));
        el.style.columnRuleWidth = '8px';
        out.push(type(declared.get('column-rule-width')), String(declared.get('column-rule-width')));
        out.join('|')
    "#);
    assert_eq!(actual, concat!(
        "CSSStyleValue|CSSStyleValue|CSSUnitValue,CSSKeywordValue|21|CSSStyleValue|1|CSSStyleValue|",
        "CSSKeywordValue|CSSStyleValue|7|CSSStyleValue|CSSStyleValue|8px"
    ));
}

#[test]
fn typed_computed_widths_and_line_height_read_the_cascaded_values() {
    let (_t, root) = page();
    let actual = eval(&root, r#"
        const el = document.body.appendChild(document.createElement('div'));
        el.style.cssText = 'zoom:2; writing-mode:vertical-rl; line-height:1.25; font-size:16px; border-block-start-width:thick;';
        const map = el.computedStyleMap(), css = getComputedStyle(el);
        [css.borderRightWidth, map.get('border-right-width').value,
         map.get('border-block-start-width').value, css.lineHeight,
         map.get('line-height').value, map.get('line-height').unit].join('|')
    "#);
    assert_eq!(actual, "0px|5|5|20px|1.25|number");
}

#[test]
fn typed_numeric_writes_preserve_units_and_math_until_replaced() {
    let (_t, root) = page();
    let actual = eval(&root, r#"
        const sheet = new CSSStyleSheet();
        sheet.replaceSync('#typed-numeric { opacity: 25%; }');
        document.adoptedStyleSheets = [sheet];
        const rule = sheet.cssRules[0], el = document.body.appendChild(document.createElement('div'));
        el.id = 'typed-numeric';
        const result = [String(rule.styleMap.get('opacity'))];
        const percentage = CSS.percent(25);
        rule.styleMap.set('opacity', percentage);
        percentage.value = 99;
        rule.style.color = 'blue';
        result.push(rule.style.opacity, String(rule.styleMap.get('opacity')), rule.cssText.includes('opacity: 25%'),
                    String(el.computedStyleMap().get('opacity')));
        rule.style.opacity = '25%';
        result.push(String(rule.styleMap.get('opacity')));
        el.attributeStyleMap.set('fill-opacity', CSS.percent(-3));
        el.style.display = 'block';
        const clone = el.cloneNode(true);
        result.push(clone.style.fillOpacity, String(clone.attributeStyleMap.get('fill-opacity')),
                    String(el.computedStyleMap().get('fill-opacity')));
        el.style.cssText = el.style.cssText;
        result.push(String(el.attributeStyleMap.get('fill-opacity')));
        const sum = new CSSMathSum(CSS.number(2), CSS.number(3));
        el.attributeStyleMap.set('orphans', sum);
        el.attributeStyleMap.set('order', CSS.number(-3.14));
        const order = el.attributeStyleMap.get('order');
        result.push(el.style.orphans, el.attributeStyleMap.get('orphans').values.length,
                    String(el.computedStyleMap().get('orphans')), order.constructor.name,
                    order.values[0].value, String(el.computedStyleMap().get('order')));
        el.style.setProperty('order', 'bogus');
        result.push(el.style.order, el.style.removeProperty('order'), el.attributeStyleMap.has('order'));
        result.join('|')
    "#);
    assert_eq!(actual, concat!(
        "0.25|25%|25%|true|0.25|0.25|-3%|-3%|0|-0.03|",
        "calc(2 + 3)|2|5|CSSMathSum|-3.14|-3|calc(-3.14)|calc(-3.14)|false"
    ));
}

#[test]
fn typed_shorthand_and_grid_line_writes_preserve_valid_declarations() {
    let (_t, root) = page();
    let actual = eval(&root, r#"
        const el = document.createElement('div'), map = el.attributeStyleMap;
        map.set('margin', '1px 2px');
        map.set('grid-row-start', 'span 2');
        const results = [];
        for (const [property, value] of [
            ['margin', CSS.px(3)], ['grid-row-start', new CSSMathSum(CSS.number(2), CSS.number(3))]
        ]) {
            try { map.set(property, value); results.push(false); }
            catch (e) { results.push(e instanceof TypeError); }
        }
        results.push(el.style.margin, el.style.gridRowStart);
        map.set('margin', CSSStyleValue.parse('margin', '4px'));
        map.set('grid-row-start', new CSSKeywordValue('auto'));
        map.set('grid-column-end', '3');
        map.set('margin-left', CSS.px(7));
        results.push(el.style.marginTop, el.style.gridRowStart, el.style.gridColumnEnd, el.style.marginLeft);
        results.join('|')
    "#);
    assert_eq!(actual, "true|true|1px 2px|span 2|4px|auto|3|7px");
}

#[test]
fn speak_keywords_cascade_and_reify_as_typed_values() {
    let (_t, root) = page();
    let actual = eval(&root, r#"
        const parent = document.body.appendChild(document.createElement('div'));
        const child = parent.appendChild(document.createElement('span'));
        const map = child.attributeStyleMap, computed = child.computedStyleMap();
        const values = [computed.get('speak').value];
        parent.style.setProperty('speak', 'never');
        values.push(computed.get('speak').value);
        map.set('speak', new CSSKeywordValue('always'));
        values.push(child.style.getPropertyValue('speak'), computed.get('speak').value);
        child.style.setProperty('speak', 'invalid');
        values.push(map.get('speak').value);
        map.delete('speak');
        values.push(computed.get('speak').value);
        values.join('|')
    "#);
    assert_eq!(actual, "auto|never|always|always|always|never");
}

#[test]
fn transition_effect_exposes_independent_computed_endpoints() {
    let (_t, root) = page();
    let actual = eval(&root, r#"
        const el = document.body.appendChild(document.createElement('div'));
        el.style.cssText = 'width: 12px; --end: 45px';
        getComputedStyle(el).width;
        el.style.transition = 'width 100s ease-in';
        el.style.width = 'var(--end)';
        const effect = el.getAnimations()[0].effect;
        const copy = new KeyframeEffect(effect);
        const first = effect.getKeyframes();
        first[0].width = '99px';
        effect.target = null;
        const frames = effect.getKeyframes();
        effect.setKeyframes([{opacity: '0'}, {opacity: '1'}]);
        [frames.length, frames[0].width, frames[0].computedOffset, frames[1].width,
         frames[1].computedOffset, frames[1].easing, frames[1].composite,
         copy.getKeyframes()[1].width, effect.getKeyframes()[1].opacity].join('|')
    "#);
    assert_eq!(actual, "2|12px|0|45px|1|linear|auto|45px|1");
}

#[test]
fn css_animation_effect_lists_computed_values_in_offset_order() {
    let (_t, root) = page();
    let actual = eval(&root, r#"
        const sheet = document.head.appendChild(document.createElement('style'));
        sheet.textContent = '@keyframes probe { to { left: var(--end); } from { left: 2em; } }';
        const el = document.body.appendChild(document.createElement('div'));
        el.style.cssText = 'position:relative;font-size:10px;--end:50px;animation:probe 100s linear';
        const effect = el.getAnimations()[0].effect, first = effect.getKeyframes();
        el.style.setProperty('--end', '60px');
        const second = effect.getKeyframes();
        const script = new KeyframeEffect(el, [{left:'1em'}, {left:'3em'}], 100);
        [first.length, first[0].offset, first[0].left, first[1].computedOffset, first[1].left,
         second[1].left, second[0].easing, second[0].composite, script.getKeyframes()[0].left].join('|')
    "#);
    assert_eq!(actual, "2|0|20px|1|50px|60px|linear|auto|1em");
}

#[test]
fn registered_computed_map_keeps_list_items_and_specified_token_streams() {
    let (_t, root) = page();
    let actual = eval(&root, r#"
        CSS.registerProperty({name:'--lengths',syntax:'<length>+',inherits:false,initialValue:'1px 2px'});
        CSS.registerProperty({name:'--any',syntax:'*',inherits:false});
        const el = document.body.appendChild(document.createElement('div'));
        el.style.cssText = 'font-size:10px; --lengths:2em 3px; --any:5px; --raw:6px';
        const map = el.computedStyleMap();
        const values = map.getAll('--lengths');
        const iterated = Array.from(map).find(([key]) => key === '--lengths')[1];
        [map.get('--lengths').value, values.map(x => x.unit).join(','), iterated.map(String).join(','),
         map.get('--any').constructor.name, map.get('--raw').constructor.name,
         el.attributeStyleMap.get('--lengths').constructor.name].join('|')
    "#);
    assert_eq!(actual, "20|px,px|20px,3px|CSSUnparsedValue|CSSUnparsedValue|CSSUnparsedValue");
}

#[test]
fn later_stylesheet_registration_wins_after_earlier_sheets_are_removed() {
    let (_t, root) = page();
    let actual = eval(&root, r#"
        const el = document.body.appendChild(document.createElement('div'));
        function sheet(text) {
            const s = document.createElement('style');
            s.textContent = text;
            document.body.append(s);
            void s.sheet.cssRules;
            return s;
        }
        const add = (name, type, initial) => sheet(
            '@property '+name+' { syntax:"<'+type+'>"; inherits:false; initial-value:'+initial+' }');
        const read = name => el.computedStyleMap().get(name);
        const prior = sheet('div {--earlier:100px}');
        const priorRegistration = add('--earlier','length','0px');
        read('--earlier');
        priorRegistration.remove();
        read('--earlier');
        prior.remove();
        sheet('div {--probe:0; --raw:0}');
        add('--probe','length','1000px');
        const result = [read('--probe').unit];
        const integer = add('--probe','integer','1000');
        result.push(read('--probe').unit,read('--raw').constructor.name);
        integer.remove();
        result.push(read('--probe').unit);
        result.join('|')
    "#);
    assert_eq!(actual, "px|number|CSSUnparsedValue|px");
}

#[test]
fn connected_stylesheet_moves_follow_document_order() {
    let (_t, root) = page();
    let actual = eval(&root, r#"
        const target = document.body.appendChild(document.createElement('div'));
        const a = document.body.appendChild(document.createElement('section'));
        const b = document.body.appendChild(document.createElement('section'));
        const add = (parent, value) => {
            const sheet = document.createElement('style');
            sheet.textContent = 'div { --winner: ' + value + ' }';
            parent.append(sheet); return sheet;
        };
        const first = add(a,'first'), second = add(a,'second');
        const read = () => getComputedStyle(target).getPropertyValue('--winner').trim();
        const result = [read()];
        a.append(first); result.push(read());
        b.append(second); result.push(read());
        document.body.append(a); result.push(read());
        result.join('|')
    "#);
    assert_eq!(actual, "second|first|second|first");
}

#[test]
fn registered_numeric_font_units_follow_the_computing_element() {
    let (_t, root) = page();
    let actual = eval(&root, r#"
        CSS.registerProperty({name:'--scale',syntax:'<number>',inherits:true,initialValue:'1'});
        const parent=document.body.appendChild(document.createElement('section'));
        const child=parent.appendChild(document.createElement('div'));
        parent.style.cssText='font-size:30px;--scale:calc(1em / 1px)';
        child.style.cssText='font-size:10px;--raw:1em;margin-left:var(--raw)';
        const cs=getComputedStyle(child);
        const values=[cs.getPropertyValue('--scale'),cs.marginLeft];
        child.style.setProperty('--scale','calc(1em / 1px)');
        values.push(cs.getPropertyValue('--scale'));
        child.style.fontSize='12px';
        values.push(cs.getPropertyValue('--scale'),cs.marginLeft);
        values.join('|')
    "#);
    assert_eq!(actual, "30|10px|10|12|12px");
}

#[test]
fn registered_font_inputs_select_inherited_metrics_only_for_used_references() {
    let (_t, root) = page();
    let actual = eval(&root, r#"
        for (const name of ['--weight','--other']) CSS.registerProperty({name,syntax:'<number>',inherits:false,initialValue:'1'});
        const p=document.body.appendChild(document.createElement('div')), c=p.appendChild(document.createElement('div'));
        p.style.fontSize='30px';
        c.style.cssText='font-size:10px;--weight:calc(1em / 1px);--other:calc(1em / 1px);--alias:var(--weight);font-weight:var(--alias,var(--other))';
        const s=getComputedStyle(c), read=()=>[s.fontWeight,s.getPropertyValue('--weight'),s.getPropertyValue('--other')].join(',');
        const values=[read()];
        c.style.fontWeight='var(--missing,var(--other))';
        values.push(read());
        c.style.fontWeight='400';
        values.push(read());
        CSS.registerProperty({name:'--length',syntax:'<length>',inherits:false,initialValue:'5px'});
        c.style.cssText='--length:2em;font-size:var(--length)';
        values.push(s.getPropertyValue('--length'),s.fontSize);
        values.join('|')
    "#);
    assert_eq!(actual, "30,30,10|30,10,30|400,10,10|5px|30px");
}
