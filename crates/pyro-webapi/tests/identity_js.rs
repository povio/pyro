use crate::support;
use pyro_webapi::*;
use support::network::*;

/// Whether the recorded request head has the header `name` (any case: hyper writes Title-Case names) with exactly `value`.
fn has_header(head: &str, name: &str, value: &str) -> bool {
    head.split("\r\n").filter_map(|line| line.split_once(": ")).any(|(n, v)| n.eq_ignore_ascii_case(name) && v == value)
}

#[test]
fn identity_is_page_scoped_validated_atomically_and_snapshotted_per_request() {
    let server = Server::new();
    let first = Env::new(&server.url);
    let second = Env::with_engine(&server.url, first.engine.clone());
    *second.page.net.borrow_mut() = first.page.net.borrow().clone();
    let default_ua = second.eval("navigator.userAgent");
    let default_platform = second.eval("navigator.platform");
    let default_languages = second.eval("navigator.languages.join(',')");
    let events = first.page.net.borrow().as_ref().unwrap().subscribe();
    first
        .page
        .set_user_agent_override(Some(UserAgentOverride {
            user_agent: "Mozilla/5.0 UA-one".into(),
            accept_language: Some("sl-SI,en;q=0.8".into()),
            platform: Some("TestPlatform".into()),
            metadata: None,
        }))
        .unwrap();
    assert_eq!(
        first.eval(
            "navigator.userAgent+'|'+navigator.appVersion+'|'+navigator.languages.join(',')+'|'+navigator.platform"
        ),
        "Mozilla/5.0 UA-one|5.0 UA-one|sl-SI,en|TestPlatform"
    );
    assert_eq!(second.eval("navigator.userAgent"), default_ua);
    assert_eq!(second.eval("navigator.languages.join(',')"), default_languages);
    assert!(first
        .page
        .set_user_agent_override(Some(UserAgentOverride {
            user_agent: "valid-but-not-applied".into(),
            accept_language: Some("en\r\nX-Bad: yes".into()),
            platform: Some("not-applied".into()),
            metadata: None,
        }))
        .is_err());
    assert_eq!(first.eval("navigator.platform"), "TestPlatform");
    first
        .eval("globalThis.result='pending';fetch('/redirect').then(r=>r.text()).then(v=>result=v,e=>result=String(e))");
    first.until("result!=='pending'");
    assert_eq!(first.eval("result"), "{\"answer\":42}");
    first.eval("result='pending';const x=new XMLHttpRequest();x.onload=()=>result=x.responseText;x.open('GET','/json');x.send()");
    // XHR has captured its defaults already; this changes only future requests.
    first
        .page
        .set_user_agent_override(Some(UserAgentOverride {
            user_agent: String::new(),
            accept_language: Some("fr-FR".into()),
            platform: None,
            metadata: None,
        }))
        .unwrap();
    first.until("result!=='pending'");
    assert_eq!(first.eval("navigator.userAgent"), default_ua);
    assert_eq!(first.eval("navigator.language"), "fr-FR");
    assert_eq!(first.eval("navigator.platform"), default_platform);
    first.eval("result='pending';fetch('/json').then(r=>r.text()).then(v=>result=v)");
    first.until("result!=='pending'");
    second.eval("globalThis.result='pending';fetch('/json').then(r=>r.text()).then(v=>result=v)");
    second.until("result!=='pending'");
    let requests = server.requests.lock().unwrap();
    assert_eq!(requests.len(), 5);
    for (_, headers, _) in &requests[..3] {
        assert!(has_header(headers, "user-agent", "Mozilla/5.0 UA-one"));
        assert!(has_header(headers, "accept-language", "sl-SI,en;q=0.8"));
    }
    assert!(has_header(&requests[3].1, "user-agent", &default_ua));
    assert!(has_header(&requests[3].1, "accept-language", "fr-FR"));
    assert!(has_header(&requests[4].1, "user-agent", &default_ua));
    assert!(!requests[4].1.contains("fr-FR"));
    drop(requests);
    let mut starts = Vec::new();
    while let Ok(event) = events.try_recv() {
        if let pyro_net::NetEvent::RequestWillBeSent { request, .. } = event {
            starts.push(request);
        }
    }
    assert_eq!(starts.len(), 5);
    assert!(starts[..3].iter().all(|r| r.headers["user-agent"] == "Mozilla/5.0 UA-one"));
    first.page.set_user_agent_override(None).unwrap();
    assert_eq!(first.eval("navigator.languages.join(',')"), default_languages);
    assert_eq!(first.eval("navigator.userAgent"), default_ua);
}

#[test]
fn browser_identity_defaults_do_not_turn_simple_cross_origin_requests_into_preflights() {
    let origin = Server::new();
    let remote = Server::new();
    let env = Env::new(&origin.url);
    env.page
        .set_user_agent_override(Some(UserAgentOverride {
            user_agent: "Page-agent".into(),
            accept_language: Some("de-DE".into()),
            platform: None,
            metadata: None,
        }))
        .unwrap();
    env.eval(&format!(
        "globalThis.result='pending';fetch('{}/cors').then(r=>r.text()).then(v=>result=v,e=>result=String(e))",
        remote.url.trim_end_matches("/page")
    ));
    env.until("result!=='pending'");
    assert_eq!(env.eval("result"), "cors");
    let requests = remote.requests.lock().unwrap();
    assert_eq!(requests.len(), 1, "browser-generated UA must not cause an OPTIONS request");
    assert!(requests[0].0.starts_with("GET "));
    assert!(has_header(&requests[0].1, "user-agent", "Page-agent"));
    assert!(has_header(&requests[0].1, "accept-language", "de-DE"));
}
