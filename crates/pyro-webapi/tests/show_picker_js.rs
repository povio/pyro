//! `showPicker()` on same-origin `<input>` and `<select>` controls (WPT
//! html/semantics/forms/the-input-element/show-picker-{disabled-readonly,user-gesture,does-not-focus},
//! the-select-element/show-picker-{disabled,user-gesture,being-rendered,being-cv-hidden,multiple,size}):
//! each probe records `navigator.userActivation.isActive` before the call, the return value or
//! exception name, and `isActive` after it. The cross-origin `SecurityError` step is not covered.

use crate::support::network::Env;

/// Trusted key input: sticky activation and five seconds of transient activation.
fn activate(env: &Env) {
    let k =
        pyro_webapi::input::KeyInput { key: "a".into(), code: "KeyA".into(), text: "a".into(), ..Default::default() };
    pyro_webapi::input::dispatch_key(&env.page, pyro_webapi::input::KeyKind::KeyDown, &k);
    pyro_webapi::input::dispatch_key(&env.page, pyro_webapi::input::KeyKind::KeyUp, &k);
}

/// `before:result:after` for `showPicker()` on the element `#id`.
fn probe(env: &Env, id: &str, with_activation: bool) -> String {
    if with_activation {
        activate(env);
    }
    env.eval(&format!(
        r#"(() => {{
          const el = document.getElementById({id:?});
          const before = navigator.userActivation.isActive;
          let result;
          try {{ result = String(el.showPicker()); }} catch (e) {{ result = e.name; }}
          return before + ":" + result + ":" + navigator.userActivation.isActive;
        }})()"#
    ))
}

const CONTROLS: &str = r#"
  document.body.innerHTML = `
    <input id=text>
    <input id=disabled-text disabled>
    <input id=disabled-button type=button disabled>
    <fieldset disabled><input id=fieldset-text></fieldset>
    <input id=readonly-text readonly>
    <input id=readonly-date type=date readonly>
    <input id=readonly-number type=number readonly>
    <input id=readonly-button type=button readonly>
    <input id=readonly-checkbox type=checkbox readonly>
    <input id=readonly-color type=color readonly>
    <input id=readonly-file type=file readonly>
    <input id=readonly-hidden type=hidden readonly>
    <input id=readonly-range type=range readonly>
    <input id=hidden type=hidden>
    <select id=select><option>a</select>
    <select id=disabled-select disabled><option>a</select>
    <select id=readonly-select readonly><option>a</select>
    <select id=none-select style="display: none"><option>a</select>
    <div id=cv style="content-visibility: hidden"><select id=cv-select><option>a</select></div>
    <select id=multiple-select multiple><option>a</select>
    <select id=size-select size=4><option>a</select>`;
  "ok"
"#;

#[test]
fn input_show_picker_checks_mutability_before_activation_and_consumes_it() {
    let env = Env::new("http://picker.test/");
    assert_eq!(env.eval(CONTROLS), "ok");
    let rows: Vec<(&str, bool, &str)> = vec![
        // Disabled: InvalidStateError, with or without activation, activation kept.
        ("disabled-text", false, "false:InvalidStateError:false"),
        ("disabled-text", true, "true:InvalidStateError:true"),
        ("disabled-button", true, "true:InvalidStateError:true"),
        ("fieldset-text", true, "true:InvalidStateError:true"),
        // Readonly where it applies: InvalidStateError, activation kept.
        ("readonly-text", true, "true:InvalidStateError:true"),
        ("readonly-date", true, "true:InvalidStateError:true"),
        ("readonly-number", true, "true:InvalidStateError:true"),
        // Readonly where it does not apply: the control is mutable, the call consumes.
        ("readonly-button", true, "true:undefined:false"),
        ("readonly-checkbox", true, "true:undefined:false"),
        ("readonly-color", true, "true:undefined:false"),
        ("readonly-file", true, "true:undefined:false"),
        ("readonly-hidden", true, "true:undefined:false"),
        ("readonly-range", true, "true:undefined:false"),
        // Mutable: NotAllowedError without activation, else the call consumes it.
        ("text", false, "false:NotAllowedError:false"),
        ("text", true, "true:undefined:false"),
        // A hidden input is not rendered and still succeeds (no being-rendered step for input).
        ("hidden", true, "true:undefined:false"),
    ];
    let got: Vec<String> = rows.iter().map(|(id, act, _)| format!("{id}/{act} {}", probe(&env, id, *act))).collect();
    let want: Vec<String> = rows.iter().map(|(id, act, want)| format!("{id}/{act} {want}")).collect();
    assert_eq!(got, want);
    // A second call in the same activation finds it consumed.
    activate(&env);
    assert_eq!(
        env.eval(
            r#"(() => {
          const t = document.getElementById("text");
          t.showPicker();
          try { t.showPicker(); return "no throw"; } catch (e) { return e.name + " " + (document.activeElement === t); }
        })()"#
        ),
        "NotAllowedError false"
    );
}

#[test]
fn select_show_picker_checks_disabled_activation_and_rendering_in_order() {
    let env = Env::new("http://picker.test/");
    assert_eq!(env.eval(CONTROLS), "ok");
    let rows: Vec<(&str, bool, &str)> = vec![
        ("disabled-select", false, "false:InvalidStateError:false"),
        ("disabled-select", true, "true:InvalidStateError:true"),
        // `readonly` does not apply to a select.
        ("readonly-select", true, "true:undefined:false"),
        ("select", false, "false:NotAllowedError:false"),
        // Not being rendered: NotSupportedError after the activation check, activation kept.
        ("none-select", false, "false:NotAllowedError:false"),
        ("none-select", true, "true:NotSupportedError:true"),
        ("cv-select", true, "true:NotSupportedError:true"),
        ("select", true, "true:undefined:false"),
        ("multiple-select", true, "true:undefined:false"),
        ("size-select", true, "true:undefined:false"),
    ];
    let got: Vec<String> = rows.iter().map(|(id, act, _)| format!("{id}/{act} {}", probe(&env, id, *act))).collect();
    let want: Vec<String> = rows.iter().map(|(id, act, want)| format!("{id}/{act} {want}")).collect();
    assert_eq!(got, want);
    // Rendered again once the styles change.
    env.eval(
        r#"document.getElementById("none-select").style.display = "inline-block";
                document.getElementById("cv").style.contentVisibility = "visible"; "ok""#,
    );
    assert_eq!(probe(&env, "none-select", true), "true:undefined:false");
    assert_eq!(probe(&env, "cv-select", true), "true:undefined:false");
}

#[test]
fn user_activation_reflects_sticky_and_transient_activation() {
    let env = Env::new("http://picker.test/");
    assert_eq!(
        env.eval(
            r#"[navigator.userActivation === navigator.userActivation,
                     navigator.userActivation instanceof UserActivation,
                     navigator.userActivation.hasBeenActive, navigator.userActivation.isActive].join()"#
        ),
        "true,true,false,false"
    );
    activate(&env);
    assert_eq!(
        env.eval("[navigator.userActivation.hasBeenActive, navigator.userActivation.isActive].join()"),
        "true,true"
    );
}
