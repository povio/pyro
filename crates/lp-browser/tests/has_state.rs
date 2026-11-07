//! `:has()` follows form and element state through the real browser: each
//! script-driven state change restyles the `:has()` anchors that can see the
//! changed element, read back through `getComputedStyle`.
use lp_browser::{NavigateOpts, StaticLoader};
use std::rc::Rc;
use std::time::{Duration, Instant};

const PAGE: &str = r#"<!doctype html><style>
#f:has(input:invalid) { color: rgb(1, 0, 0) }
label:has(input:checked) { color: rgb(2, 0, 0) }
#fd:has(:focus) { color: rgb(3, 0, 0) }
#sl:has(option:checked) { color: rgb(6, 0, 0) }
#ds:has(:disabled) { color: rgb(7, 0, 0) }
#ph:has(:placeholder-shown) { color: rgb(8, 0, 0) }
#op:has(:open) { color: rgb(9, 0, 0) }
#pp:has(:popover-open) { color: rgb(10, 0, 0) }
#dl:has(dialog:open) { color: rgb(12, 0, 0) }
#rg:has(#r2:checked) { color: rgb(13, 0, 0) }
#sib:has(+ input:checked) { color: rgb(11, 0, 0) }
#fv:has(:focus-visible) { color: rgb(14, 0, 0) }
#ui:has(:user-invalid) { color: rgb(15, 0, 0) }
#ind:has(:indeterminate) { color: rgb(16, 0, 0) }
#ro:has(:read-only) { color: rgb(17, 0, 0) }
#cs:has(:state(on)) { color: rgb(18, 0, 0) }
#uv:has(:user-valid) { color: rgb(19, 0, 0) }
#rn:has(:out-of-range) { color: rgb(20, 0, 0) }
#rn:has(:in-range) ~ #rn2 { color: rgb(21, 0, 0) }
#pg:has(progress:indeterminate) { color: rgb(22, 0, 0) }
#rq:has(:required) { color: rgb(23, 0, 0) }
#df:has(:default) { color: rgb(24, 0, 0) }
</style><body>
<form id=f><input id=fi required></form>
<label id=l><input id=cb type=checkbox></label>
<div id=fd><input id=fdi></div>
<div id=sl><select id=sel><option id=o1>a</option><option id=o2>b</option></select></div>
<div id=ds><input id=dsi></div>
<div id=ph><input id=phi placeholder=p></div>
<div id=op><details id=det><summary>s</summary>x</details></div>
<div id=pp><div id=pop popover>x</div></div>
<div id=dl><dialog id=dlg>d</dialog></div>
<div id=rg><input type=radio name=r id=r1 checked><input type=radio name=r id=r2></div>
<span id=sib>s</span><input id=cb2 type=checkbox>
<div id=fv><button id=fvb>b</button></div>
<form id=uf novalidate><div id=ui><input id=uii required></div></form>
<div id=ind><input id=indi type=checkbox></div>
<div id=ro><input id=roi></div>
<div id=cs><x-st id=xst></x-st></div>
<form id=vf><div id=uv><input id=uvi required></div></form>
<div id=rn><input id=rni type=number min=1 max=10 value=5></div><p id=rn2>r</p>
<div id=pg><progress id=pgi value=50 max=100></progress></div>
<div id=rq><input id=rqi></div>
<div id=df><input id=dfi type=checkbox></div>
<script>
customElements.define('x-st', class extends HTMLElement {
  constructor() { super(); this.i = this.attachInternals(); }
});
const color = (id) => getComputedStyle(document.getElementById(id)).color;
globalThis.fails = [];
globalThis.expect = (label, id, want) => {
  const got = color(id);
  if (got !== want) fails.push(`${label}: #${id} ${got} != ${want}`);
};
</script>"#;

const STEPS: &str = r#"
const $ = (id) => document.getElementById(id);
const black = 'rgb(0, 0, 0)';
expect('invalid initially', 'f', 'rgb(1, 0, 0)');
$('fi').value = 'x';
expect('value makes it valid', 'f', black);
$('fi').value = '';
expect('value makes it invalid', 'f', 'rgb(1, 0, 0)');

expect('unchecked', 'l', black);
$('cb').checked = true;
expect('checked', 'l', 'rgb(2, 0, 0)');
$('cb').click();
expect('click unchecks', 'l', black);

expect('no focus', 'fd', black);
$('fdi').focus();
expect('focus', 'fd', 'rgb(3, 0, 0)');
$('fdi').blur();
expect('blur', 'fd', black);

expect('option checked by default', 'sl', 'rgb(6, 0, 0)');
$('sel').selectedIndex = -1;
expect('no option selected', 'sl', black);
$('sel').value = 'b';
expect('option selected again', 'sl', 'rgb(6, 0, 0)');

$('dsi').disabled = true;
expect('disabled', 'ds', 'rgb(7, 0, 0)');
$('dsi').disabled = false;
expect('enabled', 'ds', black);

expect('placeholder shown', 'ph', 'rgb(8, 0, 0)');
$('phi').value = 'v';
expect('placeholder hidden', 'ph', black);

$('det').open = true;
expect('details open', 'op', 'rgb(9, 0, 0)');
$('det').open = false;
expect('details closed', 'op', black);

$('pop').showPopover();
expect('popover open', 'pp', 'rgb(10, 0, 0)');
$('pop').hidePopover();
expect('popover closed', 'pp', black);

$('dlg').show();
expect('dialog open', 'dl', 'rgb(12, 0, 0)');
$('dlg').close();
expect('dialog closed', 'dl', black);

expect('radio 2 unchecked', 'rg', black);
$('r2').checked = true;
expect('radio 2 checked', 'rg', 'rgb(13, 0, 0)');
$('r1').checked = true;
expect('radio 1 unchecks radio 2', 'rg', black);

$('cb2').checked = true;
expect('later sibling checked', 'sib', 'rgb(11, 0, 0)');

$('indi').indeterminate = true;
expect('indeterminate', 'ind', 'rgb(16, 0, 0)');
$('indi').indeterminate = false;
expect('determinate', 'ind', black);

expect('read-write', 'ro', black);
$('roi').readOnly = true;
expect('read-only', 'ro', 'rgb(17, 0, 0)');

$('xst').i.states.add('on');
expect('custom state on', 'cs', 'rgb(18, 0, 0)');
$('xst').i.states.delete('on');
expect('custom state off', 'cs', black);

expect('not user-invalid before interaction', 'ui', black);
$('uf').noValidate = false;
$('uf').requestSubmit();
expect('user-invalid after a submit attempt', 'ui', 'rgb(15, 0, 0)');
if (!$('uii').matches(':user-invalid') || $('uii').matches(':user-valid')) fails.push('matches(:user-invalid)');
if (document.querySelector('#ui :user-invalid') !== $('uii')) fails.push('querySelector(:user-invalid)');
$('uii').value = 'ok';
expect('user-valid once filled', 'ui', black);
$('uf').reset();
expect('reset clears user validity', 'ui', black);

$('vf').requestSubmit();
expect('user-invalid is not user-valid', 'uv', black);
$('uvi').value = 'ok';
expect('user-valid once valid', 'uv', 'rgb(19, 0, 0)');

expect('in range', 'rn2', 'rgb(21, 0, 0)');
if (!$('rni').matches(':in-range') || $('rni').matches(':out-of-range')) fails.push('matches(:in-range)');
$('rni').value = '12';
expect('out of range', 'rn', 'rgb(20, 0, 0)');
expect('not in range', 'rn2', black);
$('rni').readOnly = true;
expect('a read-only input is not validated, so neither', 'rn', black);

expect('progress with a value', 'pg', black);
$('pgi').removeAttribute('value');
expect('progress without a value', 'pg', 'rgb(22, 0, 0)');

$('rqi').required = true;
expect('required', 'rq', 'rgb(23, 0, 0)');

$('dfi').checked = true;
expect('checked is not default', 'df', black);
$('dfi').setAttribute('checked', '');
expect('default', 'df', 'rgb(24, 0, 0)');
$('dfi').checked = false;
expect('still default', 'df', 'rgb(24, 0, 0)');
JSON.stringify(fails)
"#;

#[test]
fn has_follows_script_driven_state() {
    let mut thread = crate::browser_thread().unwrap();
    let page = thread.browser.create_page().unwrap();
    let loader = Rc::new(StaticLoader::default());
    loader.insert("http://site.test/", "text/html", PAGE, Duration::ZERO);
    thread.browser.set_loader(Some(loader));
    thread.browser.navigate(page, NavigateOpts::address_bar("http://site.test/")).unwrap();
    assert_ne!(thread.run_until_idle(Some(Instant::now() + Duration::from_secs(3))), lp_runtime::RunOutcome::TimedOut);
    let realm = thread.browser.page(page).unwrap().main_frame.main_realm().unwrap();
    let scope = realm.scope().unwrap();
    let out = scope
        .eval(STEPS, None, 1)
        .unwrap_or_else(|e| panic!("{}", e.details(&scope).text()))
        .to_string()
        .unwrap();
    assert_eq!(out, "[]");
}
