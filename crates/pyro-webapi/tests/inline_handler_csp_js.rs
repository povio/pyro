//! Inline event handler attributes under CSP `script-src-attr`: the check runs when the attribute
//! is set (CSP3 "Should element's inline type behavior be blocked?"), once, whatever reads the
//! handler later; a policy that arrives after the attribute leaves it working (Chrome).

use std::time::Duration;

use crate::support::network::Env;

/// An `Env` whose document has the enforced policy `policy`, and a counter of the
/// `securitypolicyviolation` events it gets (`window.__v`, each entry `blockedURI|target id`).
fn env_with_policy(policy: &str) -> Env {
    let env = Env::new("http://csp.test/");
    env.page.dom.borrow_mut().set_trusted_types_policies(env.page.document(), policy, "").unwrap();
    env.eval(
        r#"(() => {
          window.__v = [];
          document.addEventListener("securitypolicyviolation", e => {
            __v.push(e.blockedURI + "|" + (e.target && e.target.id ? e.target.id : String(e.target === document ? "document" : e.target)));
          });
          return "ok";
        })()"#,
    );
    env
}

#[test]
fn a_blocked_attribute_is_null_at_once_and_reports_exactly_once() {
    let env = env_with_policy("script-src-attr 'none'");
    assert_eq!(
        env.eval(
            r#"(() => {
              const d = document.createElement("div");
              d.id = "d";
              d.setAttribute("onclick", "void 0");
              // Reading the handler again checks nothing and reports nothing.
              return [d.onclick === null, d.onclick === null, __v.length].join("|");
            })()"#
        ),
        "true|true|0"
    );
    env.drain_for(Duration::from_millis(300));
    // One violation, queued when the attribute was set: the element is disconnected, so the document.
    assert_eq!(env.eval(r#"__v.join(",")"#), "inline|document");
}

#[test]
fn an_attribute_a_fragment_parse_made_is_checked_and_targets_its_element() {
    let env = env_with_policy("script-src-attr 'none'");
    assert_eq!(
        env.eval(
            r#"(() => {
              document.body.innerHTML = '<a id="t" onclick="void 0">x</a>';
              const a = document.getElementById("t");
              return [a.onclick === null, a.onclick === null].join("|");
            })()"#
        ),
        "true|true"
    );
    env.drain_for(Duration::from_millis(300));
    assert_eq!(env.eval(r#"__v.join(",")"#), "inline|t");
}

#[test]
fn a_policy_that_arrives_after_the_attribute_does_not_unset_the_handler() {
    let env = Env::new("http://csp.test/");
    assert_eq!(
        env.eval(
            r#"(() => {
              const d = document.createElement("div");
              d.setAttribute("onclick", "void 0");
              window.__d = d;
              return "set";
            })()"#
        ),
        "set"
    );
    env.page.dom.borrow_mut().set_trusted_types_policies(env.page.document(), "script-src-attr 'none'", "").unwrap();
    // The attribute was allowed when it was set: Chrome keeps the handler, and reports nothing.
    assert_eq!(env.eval(r#"typeof __d.onclick"#), "function");
}

#[test]
fn an_allowed_attribute_reports_nothing() {
    let env = env_with_policy("script-src-attr 'unsafe-inline'");
    assert_eq!(
        env.eval(
            r#"(() => {
              const d = document.createElement("div");
              d.setAttribute("onclick", "void 0");
              return typeof d.onclick;
            })()"#
        ),
        "function"
    );
    env.drain_for(Duration::from_millis(200));
    assert_eq!(env.eval("__v.length"), "0");
}
