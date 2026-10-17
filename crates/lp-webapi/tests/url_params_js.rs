mod support;
use support::network::*;

#[test]
fn params_fetch_and_xhr_use_native_serialization_and_snapshot_at_send() {
    let server = Server::new();
    let env = Env::new(&server.url);
    env.eval(
        r#"globalThis.result='pending';globalThis.params=new URLSearchParams([['a','two words'],['a','+']]);
      params.toString=()=>{throw Error('author override must not replace BodyInit serialization')};
      const pending=fetch('/echo',{method:'POST',body:params});params.append('late','change');
      pending.then(r=>r.text()).then(v=>result=v,e=>result='ERROR '+e);"#,
    );
    env.until("result!=='pending'");
    assert_eq!(env.eval("result"), "a=two+words&a=%2B");
    env.eval(
        r#"result='pending';const xhr=new XMLHttpRequest();xhr.open('POST','/echo');
      xhr.onload=()=>result=xhr.responseText;xhr.onerror=()=>result='error';xhr.send(params);params.delete('a');"#,
    );
    env.until("result!=='pending'");
    assert_eq!(env.eval("result"), "a=two+words&a=%2B&late=change");
    env.eval(r#"result='pending';fetch('/echo',{method:'POST',body:params,headers:{'Content-Type':'text/custom'}}).then(r=>r.text()).then(v=>result=v,e=>result='ERROR '+e)"#);
    env.until("result!=='pending'");
    assert_eq!(env.eval("result"), "late=change");
    let requests = server.requests.lock().unwrap();
    assert_eq!(requests.len(), 3);
    for request in &requests[..2] {
        assert!(request
            .1
            .to_ascii_lowercase()
            .contains("content-type: application/x-www-form-urlencoded;charset=utf-8\r\n"));
    }
    assert!(requests[2].1.contains("content-type: text/custom\r\n"));
}
