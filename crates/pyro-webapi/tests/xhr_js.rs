use crate::support;
use pyro_webapi::*;
use std::time::Duration;
use support::network::*;

#[test]
fn xhr_json_lifecycle_handler_order_and_network_correlation() {
    let server = Server::new();
    let env = Env::new(&server.url);
    env.page.set_request_context(RequestContext {
        frame_id: Some("xhr-frame".into()),
        loader_id: Some("xhr-loader".into()),
    });
    let events = env.page.net.borrow().as_ref().unwrap().subscribe();
    env.eval(r#"globalThis.result='pending';globalThis.log=[];globalThis.x=new XMLHttpRequest();
        if(!(x instanceof EventTarget)||x.readyState!==0||x.status!==0||x.responseText!==''||x.onload!==null)throw Error('initial');
        x.onreadystatechange=()=>log.push('state'+x.readyState);
        x.onloadstart=()=>log.push('start');
        x.addEventListener('load',()=>log.push('first'));
        x.onload=()=>log.push('old');
        x.addEventListener('load',()=>log.push('last'));
        x.onload=()=>{log.push('handler');if(x.status!==200||x.response.answer!==42||x.response!==x.response)throw Error('json');};
        x.onloadend=()=>result=log.join(',');x.onerror=()=>result='error';
        x.open('get','/json');x.responseType='json';x.send();"#);
    env.until("result!=='pending'");
    assert_eq!(env.eval("result"), "state1,start,state2,state3,state4,first,handler,last");
    assert_eq!(env.eval("x.getResponseHeader('Content-Type')"), "application/json");
    assert_eq!(env.eval("x.getResponseHeader('set-cookie')"), "null");
    assert_eq!(env.eval("try{x.responseText;'bad'}catch(e){e.name}"), "InvalidStateError");
    assert_eq!(env.page.xhr_usage(), (0, 13));
    let mut starts = 0;
    while let Ok(event) = events.try_recv() {
        if let pyro_net::NetEvent::RequestWillBeSent { request, .. } = event {
            starts += 1;
            assert_eq!(request.destination, pyro_net::Destination::Xhr);
            assert_eq!(request.frame_id.as_deref(), Some("xhr-frame"));
            assert_eq!(request.loader_id.as_deref(), Some("xhr-loader"));
        }
    }
    assert_eq!(starts, 1);
}

#[test]
fn xhr_posts_subview_combines_headers_and_reuses_after_completion() {
    let server = Server::new();
    let env = Env::new(&server.url);
    env.eval(r#"globalThis.result='pending';globalThis.x=new XMLHttpRequest();let n=0;
        x.onloadend=()=>{if(++n===1){if(x.responseText!=='AB')return result='body';x.open('GET','/missing');x.send({toString(){throw Error('GET body converted')}})}else{result=x.status===404&&x.responseText==='missing'?'ok':'404'}};
        x.open('post','/echo');x.setRequestHeader('x-test','a');x.setRequestHeader('X-Test','b');x.setRequestHeader('Cookie','wrong=1');
        x.send(new Uint8Array([0,65,66,0]).subarray(1,3));"#);
    env.until("result!=='pending'");
    assert_eq!(env.eval("result"), "ok");
    let requests = server.requests.lock().unwrap();
    assert!(requests[0].0.starts_with("POST "));
    assert_eq!(requests[0].2, b"AB");
    assert!(requests[0].1.contains("x-test: a, b"));
    assert!(!requests[0].1.contains("wrong"));
}

#[test]
fn xhr_abort_at_headers_fires_once_and_resets() {
    let server = Server::new();
    let env = Env::new(&server.url);
    env.eval(r#"globalThis.result='pending';globalThis.log=[];globalThis.x=new XMLHttpRequest();
        x.onreadystatechange=()=>{log.push(x.readyState);if(x.readyState===2)x.abort()};
        x.onabort=()=>log.push('abort');x.onload=()=>log.push('BADload');x.onerror=()=>log.push('BADerror');x.onloadend=()=>{log.push('end');result='done'};
        x.open('GET','/gated');x.send();"#);
    env.until("result!=='pending'");
    assert_eq!(env.eval("log.join(',')"), "1,2,4,abort,end");
    assert_eq!(env.eval("x.readyState+':'+x.status+':'+x.responseText"), "0:0:");
    assert_eq!(env.page.xhr_usage(), (0, 0));
    server.release.send(()).unwrap();
    while let Ok(task) = env.tasks.try_recv() {
        task();
    }
    assert_eq!(env.eval("log.join(',')"), "1,2,4,abort,end");
}

#[test]
fn xhr_reopening_in_callback_suppresses_old_completion() {
    let server = Server::new();
    let env = Env::new(&server.url);
    env.eval(
        r#"globalThis.result='pending';globalThis.x=new XMLHttpRequest();let switched=false;let loads=0;
        x.onreadystatechange=()=>{if(x.readyState===2&&!switched){switched=true;x.open('GET','/missing');x.send()}};
        x.onload=()=>{loads++;result=loads===1&&x.status===404&&x.responseText==='missing'?'ok':'wrong'};
        x.open('GET','/json');x.send();"#,
    );
    env.until("result!=='pending'");
    assert_eq!(env.eval("result"), "ok");
}

#[test]
fn xhr_timeout_uses_captured_timer_and_is_measured_from_send() {
    let server = Server::new();
    let env = Env::new(&server.url);
    env.eval(r#"globalThis.result='pending';globalThis.x=new XMLHttpRequest();globalThis.log=[];
        x.timeout=20;x.ontimeout=()=>log.push('timeout');x.onabort=()=>log.push('BADabort');x.onerror=()=>log.push('BADerror');x.onloadend=()=>result=log.join(',');
        globalThis.setTimeout=()=>{throw Error('replaced timer')};globalThis.clearTimeout=()=>{throw Error('replaced timer')};
        x.open('GET','/gated');x.send();"#);
    env.until("result!=='pending'");
    assert_eq!(env.eval("result"), "timeout");
    assert_eq!(env.eval("x.readyState+':'+x.status"), "4:0");
    assert_eq!(env.page.xhr_usage(), (0, 0));
    server.release.send(()).unwrap();
}

#[test]
fn xhr_limits_error_and_page_disposal_suppress_queued_events() {
    let server = Server::new();
    let env = Env::new(&server.url);
    assert!(env.page.set_xhr_limits(XhrLimits { max_body_bytes: 32, ..Default::default() }));
    env.eval("globalThis.result='pending';globalThis.x=new XMLHttpRequest();x.onerror=()=>result='error';x.onload=()=>result='BADload';x.open('GET','/large');x.send()");
    env.until("result!=='pending'");
    assert_eq!(env.eval("result"), "error");
    assert_eq!(env.page.xhr_usage(), (0, 0));
    env.eval("result='pending';x.open('GET','/gated');x.send()");
    let queued = env.tasks.recv_timeout(Duration::from_secs(5)).unwrap();
    env.page.dispose();
    queued();
    assert_eq!(env.eval("result"), "pending");
    assert_eq!(env.page.xhr_usage(), (0, 0));
    server.release.send(()).unwrap();
}

#[test]
fn xhr_partial_text_decoder_preserves_split_utf8_and_charset() {
    let server = Server::new();
    let env = Env::new(&server.url);
    env.eval("globalThis.result='pending';globalThis.x=new XMLHttpRequest();x.onreadystatechange=()=>{if(x.readyState===3)result='partial'};x.onload=()=>result=x.responseText;x.open('GET','/split-utf8');x.send()");
    env.until("result!=='pending'");
    assert_eq!(env.eval("result"), "partial");
    assert_eq!(env.eval("x.responseText"), "");
    server.release.send(()).unwrap();
    env.until("result!=='partial'");
    assert_eq!(env.eval("result"), "€");
    env.eval("result='pending';x.onreadystatechange=null;x.open('GET','/latin1');x.onload=()=>result=x.responseText;x.send()");
    env.until("result!=='pending'");
    assert_eq!(env.eval("result"), "é");
}

#[test]
fn xhr_binary_json_and_invalid_state_contracts() {
    let server = Server::new();
    let env = Env::new(&server.url);
    assert_eq!(
        env.eval(
            r#"(()=>{const x=new XMLHttpRequest();let errors=[];
        for(const f of [()=>x.send(),()=>x.setRequestHeader('a','b')]){try{f()}catch(e){errors.push(e.name)}}
        try{x.open('TRACE','/json')}catch(e){errors.push(e.name)}
        x.open('GET','/json');try{x.setRequestHeader('bad name','v')}catch(e){errors.push(e.name)}
        return errors.join(',')})()"#
        ),
        "InvalidStateError,InvalidStateError,SecurityError,SyntaxError"
    );
    env.eval(r#"globalThis.result='pending';globalThis.x=new XMLHttpRequest();let phase=0;
        x.onload=()=>{
            if(phase===0){if(!(x.response instanceof ArrayBuffer)||x.response.byteLength!==13||x.response!==x.response)return result='buffer';phase++;x.open('GET','/json');x.responseType='blob';x.send()}
            else if(phase===1){if(!(x.response instanceof Blob)||x.response.size!==13||x.response.type!=='application/json'||x.response!==x.response)return result='blob';phase++;x.open('GET','/bad-json');x.responseType='json';x.send()}
            else result=x.response===null?'ok':'json';
        };x.open('GET','/json');x.responseType='arraybuffer';x.send();"#);
    env.until("result!=='pending'");
    assert_eq!(env.eval("result"), "ok");
}

#[test]
fn xhr_cors_headers_and_failure_have_distinct_events() {
    let server = Server::new();
    let other = Server::new();
    let env = Env::new(&server.url);
    env.eval(&format!("globalThis.remote='{}'", other.url.replace("/page", "")));
    env.eval(r#"globalThis.result='pending';globalThis.x=new XMLHttpRequest();let phase=0;
        x.onload=()=>{if(phase++===0){if(x.getResponseHeader('X-Visible')!=='yes'||x.getResponseHeader('X-Second')!=='second'||x.getResponseHeader('X-Hidden')!==null||x.getResponseHeader('Set-Cookie')!==null)return result='headers';x.open('GET',remote+'/json');x.send()}else result='BADcors'};
        x.onerror=()=>result=x.status===0&&x.responseText===''?'ok':'error state';x.open('GET',remote+'/cors');x.send();"#);
    env.until("result!=='pending'");
    assert_eq!(env.eval("result"), "ok");
}

#[test]
fn xhr_reentrant_body_conversion_and_loadstart_abort_do_not_send() {
    let server = Server::new();
    let env = Env::new(&server.url);
    assert_eq!(env.eval(r#"(()=>{const x=new XMLHttpRequest();x.open('POST','/echo');try{x.send({toString(){x.open('GET','/json');return 'old'}})}catch(e){return e.name}})()"#),"InvalidStateError");
    env.eval(r#"globalThis.result='pending';globalThis.x=new XMLHttpRequest();x.onloadstart=()=>x.abort();x.onabort=()=>result='aborted';x.open('POST','/echo');x.send('body');"#);
    assert_eq!(env.eval("result"), "aborted");
    assert_eq!(env.page.xhr_usage(), (0, 0));
    assert!(server.requests.lock().unwrap().is_empty());
}

#[test]
fn xhr_timeout_update_uses_original_start_and_can_be_disabled() {
    let server = Server::new();
    let env = Env::new(&server.url);
    env.eval(r#"globalThis.result='pending';globalThis.x=new XMLHttpRequest();x.ontimeout=()=>result='timeout';x.open('GET','/gated');x.timeout=1000;x.send();setTimeout(()=>x.timeout=5,30);"#);
    let start = std::time::Instant::now();
    env.until("result!=='pending'");
    assert_eq!(env.eval("result"), "timeout");
    assert!(start.elapsed() < Duration::from_millis(900));
    server.release.send(()).unwrap();
    env.eval("result='pending';x.open('GET','/json');x.timeout=1;x.send();x.timeout=0;x.onload=()=>result='loaded'");
    env.until("result!=='pending'");
    assert_eq!(env.eval("result"), "loaded");
}

#[test]
fn xhr_completed_buffer_and_listener_cycles_are_collectable() {
    let server = Server::new();
    let env = Env::new(&server.url);
    env.eval(r#"globalThis.result='pending';(()=>{const x=new XMLHttpRequest();x.onload=()=>{if(x.responseText.length===13)result='ok'};x.open('GET','/json');x.send()})();"#);
    env.until("result!=='pending'");
    assert_eq!(env.eval("result"), "ok");
    for _ in 0..6 {
        env.eval("Array.from({length:2000},(_,i)=>({i}))");
        env.engine.collect_garbage_now();
        env.engine.drain_finalized();
        env.page.after_task();
    }
    assert_eq!(env.page.xhr_usage(), (0, 0));
    assert!(env.page.set_xhr_limits(XhrLimits::default()));
}

#[test]
fn xhr_aggregate_budget_and_pending_cap_refund_on_reopen_and_abort() {
    let server = Server::new();
    let env = Env::new(&server.url);
    assert!(env.page.set_xhr_limits(XhrLimits { max_requests: 1, max_total_body_bytes: 20, ..Default::default() }));
    env.eval("globalThis.result='pending';globalThis.a=new XMLHttpRequest();a.open('GET','/json');a.onload=()=>result='first';a.send()");
    env.until("result!=='pending'");
    assert_eq!(env.page.xhr_usage(), (0, 13));
    env.eval("result='pending';globalThis.b=new XMLHttpRequest();b.open('GET','/json');b.onerror=()=>result='overflow';b.send()");
    env.until("result!=='pending'");
    assert_eq!(env.eval("result"), "overflow");
    assert_eq!(env.page.xhr_usage(), (0, 13));
    env.eval("a.open('GET','/gated');a.onload=null;a.send()");
    assert_eq!(env.eval("try{b.open('GET','/json');b.send();'bad'}catch(e){e.name}"), "QuotaExceededError");
    env.eval("a.abort()");
    assert_eq!(env.page.xhr_usage(), (0, 0));
    server.release.send(()).unwrap();
    env.eval("result='pending';b.open('GET','/json');b.onload=()=>result=b.responseText;b.send()");
    env.until("result!=='pending'");
    assert_eq!(env.eval("result"), "{\"answer\":42}");
}

#[test]
fn xhr_disposal_is_scoped_and_queued_callbacks_release_the_page() {
    for whole_page in [false, true] {
        let server = Server::new();
        let first = Env::new(&server.url);
        let second = Env::with_engine(&server.url, first.engine.clone());
        *second.page.net.borrow_mut() = first.page.net.borrow().clone();
        first.eval("globalThis.result='pending';globalThis.x=new XMLHttpRequest();x.timeout=30;x.onload=x.onerror=x.ontimeout=x.onabort=()=>result='stale';x.open('GET','/gated');x.send()");
        let stale = first.tasks.recv_timeout(Duration::from_secs(5)).unwrap();
        if whole_page {
            first.page.dispose();
        } else {
            lifecycle::realm_disposed(&first.page, first.realm.id());
        }
        stale();
        assert_eq!(first.eval("result"), "pending");
        assert_eq!(first.page.xhr_usage(), (0, 0));
        let weak = std::rc::Rc::downgrade(&first.page);
        drop(first);
        for _ in 0..6 {
            second.engine.collect_garbage_now();
            second.engine.drain_finalized();
        }
        assert!(weak.upgrade().is_none());
        server.release.send(()).unwrap();
        second.eval("globalThis.result='pending';const x=new XMLHttpRequest();x.onload=()=>result=x.responseText;x.open('GET','/json');x.send()");
        second.until("result!=='pending'");
        assert_eq!(second.eval("result"), "{\"answer\":42}");
    }
}

#[test]
fn xhr_text_read_is_safe_when_network_failure_precedes_its_browser_task() {
    let server = Server::new();
    let env = Env::new(&server.url);
    assert!(env.page.set_xhr_limits(XhrLimits { max_body_bytes: 2, ..Default::default() }));
    env.eval("globalThis.result='pending';globalThis.x=new XMLHttpRequest();x.onerror=()=>result='error';x.open('GET','/split-utf8');x.send()");
    env.until("x.readyState===3");
    assert_eq!(env.eval("x.responseText"), "");
    server.release.send(()).unwrap();
    let task = env.tasks.recv_timeout(Duration::from_secs(5)).expect("failed delivery queued");
    // The I/O thread has recorded the overflow but keeps the delivered buffer
    // (and its budget charge) until this queued failure is dispatched; JS still
    // observes LOADING and only the owner-visible snapshot.
    assert_eq!(env.eval("x.responseText"), "");
    task();
    env.until("result==='error'");
    assert_eq!(env.page.xhr_usage(), (0, 0));
    // pyro-net enforces the 2-byte request max_body and reports a real transport
    // Failed (also published as Network.loadingFailed), so the frontier is
    // terminal. Consumer-local overflow without a transport failure is covered
    // by the delivery unit test.
    assert_eq!(env.frontiers().last(), Some(&(true, 2, true)), "{:?}", env.log.borrow());
}

fn assert_xhr_owner_delivery(path: &str, expected: &str) {
    let server = Server::new();
    let env = Env::new(&server.url);
    let events = env.page.net.borrow().as_ref().unwrap().subscribe();
    env.eval(&format!("globalThis.result='pending';globalThis.x=new XMLHttpRequest();x.onload=()=>result=x.responseText;x.onerror=()=>result='error';x.open('GET','{path}');x.send()"));
    env.until("x.readyState===3");
    assert_eq!(env.eval("x.responseText"), "first");
    server.release.send(()).unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        match events.try_recv() {
            Ok(pyro_net::NetEvent::LoadingFinished { .. } | pyro_net::NetEvent::LoadingFailed { .. }) => break,
            Ok(_) => (),
            Err(pyro_net::SubscriptionError::Empty) => std::thread::sleep(Duration::from_millis(1)),
            Err(e) => panic!("unexpected observer state: {e:?}"),
        }
        assert!(std::time::Instant::now() < deadline, "transport did not complete");
    }
    assert_eq!(
        env.eval("x.responseText"),
        "first",
        "IO must not expose new bytes or erase delivered text before the owner task"
    );
    assert_eq!(env.eval("result"), "pending");
    env.until("result!=='pending'");
    assert_eq!(env.eval("result"), expected);
}

#[test]
fn xhr_bytes_wait_for_owner_delivery() {
    assert_xhr_owner_delivery("/gated-text", "firstlater");
}

#[test]
fn xhr_error_waits_for_owner_delivery() {
    assert_xhr_owner_delivery("/gated-failure", "error");
}

#[test]
fn xhr_delivery_frontier_reaches_the_hook_before_ready_state_changes() {
    let server = Server::new();
    let env = Env::new(&server.url);
    env.eval("globalThis.x=new XMLHttpRequest();x.onreadystatechange=()=>__log('rs'+x.readyState+' '+x.responseText);x.open('GET','/gated-text');x.send()");
    env.until("x.readyState===3");
    server.release.send(()).unwrap();
    env.until("x.readyState===4");
    let frontiers = env.frontiers();
    assert!(frontiers.first().is_some_and(|f| f.0 && !f.2), "{frontiers:?}");
    assert_eq!(frontiers.last(), Some(&(true, 10, true)));
    let first = env.log_index(|l| l.starts_with("delivery ")).unwrap();
    let terminal = env.log_index(|l| l.starts_with("delivery ") && l.ends_with("t=true")).unwrap();
    assert!(first < env.log_index(|l| l.starts_with("rs2")).unwrap(), "{:?}", env.log.borrow());
    assert!(terminal < env.log_index(|l| l == "rs4 firstlater").unwrap(), "{:?}", env.log.borrow());
}

#[test]
fn xhr_upload_object_identity_and_type() {
    let server = Server::new();
    let env = Env::new(&server.url);
    assert_eq!(
        env.eval("const x=new XMLHttpRequest();[x.upload===x.upload,x.upload instanceof XMLHttpRequestUpload,x.upload instanceof EventTarget,typeof x.upload.onprogress,(()=>{try{new XMLHttpRequestUpload();return 'constructed'}catch(e){return e.name}})()].join()"),
        "true,true,true,object,TypeError"
    );
}

const UPLOAD_LOG: &str = "globalThis.log=[];globalThis.x=new XMLHttpRequest();\
    const rec=(who)=>(e)=>log.push(who+':'+e.type+(e.type==='progress'||who==='u'?'('+e.loaded+'/'+e.total+','+e.lengthComputable+')':'')+(who==='u'&&e.target!==x.upload?'!target':''));\
    for(const t of ['loadstart','progress','abort','error','timeout','load','loadend'])x.addEventListener(t,rec('x'));\
    x.onreadystatechange=()=>log.push('rs'+x.readyState);";

fn wait_for_upload_test_transport(env: &Env) {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        if env.page.net.borrow().as_ref().unwrap().activity("upload-progress-test").is_some_and(|a| a.inflight == 0) {
            return;
        }
        assert!(std::time::Instant::now() < deadline, "transport did not finish");
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn xhr_upload_events_fire_before_headers_for_a_request_body() {
    let server = Server::new();
    let env = Env::new(&server.url);
    env.page.set_request_context(RequestContext { loader_id: Some("upload-progress-test".into()), frame_id: None });
    env.eval(UPLOAD_LOG);
    env.eval("x.open('POST','/echo');x.upload.onprogress=rec('u');for(const t of ['loadstart','load','loadend'])x.upload.addEventListener(t,rec('u'));x.send('abcdef')");
    // This assertion describes coalesced delivery. Hold the owner queue until
    // the transport's sink has completed; a separate regression covers a data
    // pump followed by completion with the same final byte count.
    wait_for_upload_test_transport(&env);
    env.until("x.readyState===4");
    assert_eq!(
        env.eval("log.join(' ')"),
        "rs1 x:loadstart u:loadstart(0/6,true) u:progress(6/6,true) u:load(6/6,true) u:loadend(6/6,true) rs2 rs3 x:progress(6/6,true) rs4 x:load x:loadend"
    );
}

#[test]
fn xhr_progress_count_tracks_owner_delivery_not_new_byte_counts() {
    for split_delivery in [false, true] {
        let server = Server::new();
        let env = Env::new(&server.url);
        env.page.set_request_context(RequestContext { loader_id: Some("upload-progress-test".into()), frame_id: None });
        env.eval(UPLOAD_LOG);
        env.eval("x.open('POST','/gated-upload-completion');x.upload.onprogress=rec('u');for(const t of ['loadstart','load','loadend'])x.upload.addEventListener(t,rec('u'));x.send('abcdef')");
        if split_delivery {
            env.until("log.includes('x:progress(6/0,false)')");
            assert_eq!(env.eval("x.readyState"), "3", "terminal chunk is still gated");
        }
        server.release.send(()).unwrap();
        if !split_delivery {
            wait_for_upload_test_transport(&env);
        }
        env.until("x.readyState===4");
        let final_progress = if split_delivery { " x:progress(6/0,false)" } else { "" };
        let expected = format!("rs1 x:loadstart u:loadstart(0/6,true) u:progress(6/6,true) u:load(6/6,true) u:loadend(6/6,true) rs2 rs3 x:progress(6/0,false){final_progress} rs4 x:load x:loadend");
        assert_eq!(env.eval("log.join(' ')"), expected);
        assert_eq!(env.eval("x.responseText"), "abcdef");
    }
}

#[test]
fn xhr_upload_events_are_skipped_without_body_or_early_listeners() {
    let server = Server::new();
    let env = Env::new(&server.url);
    env.eval(UPLOAD_LOG);
    // GET has no body: the upload complete flag is set at send.
    env.eval("x.open('GET','/json');x.upload.onloadstart=rec('u');x.upload.onload=rec('u');x.send()");
    env.until("x.readyState===4");
    assert!(!env.eval("log.join(' ')").contains("u:"), "{}", env.eval("log.join(' ')"));
    // Listeners added after send() do not set the upload listener flag.
    env.eval("log.length=0;globalThis.y=new XMLHttpRequest();y.onloadend=()=>log.push('y:loadend');y.open('POST','/echo');y.send('zz');y.upload.onload=()=>log.push('late upload listener')");
    env.until("log.includes('y:loadend')");
    assert_eq!(env.eval("log.join(' ')"), "y:loadend");
}

#[test]
fn xhr_upload_hears_errors_and_aborts_before_the_request_object() {
    let server = Server::new();
    let env = Env::new(&server.url);
    // A refused connection: request error steps.
    let closed = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    env.eval(UPLOAD_LOG);
    env.eval(&format!("x.open('POST','http://127.0.0.1:{closed}/x');for(const t of ['loadstart','error','loadend'])x.upload.addEventListener(t,rec('u'));x.send('abc')"));
    env.until("x.readyState===4");
    let log = env.eval("log.join(' ')");
    assert_eq!(
        log,
        "rs1 x:loadstart u:loadstart(0/3,true) rs4 u:error(0/0,false) u:loadend(0/0,false) x:error x:loadend"
    );
    // Abort while the request is held: same ordering with abort.
    env.eval("log.length=0;globalThis.x=new XMLHttpRequest();for(const t of ['abort','loadend'])x.addEventListener(t,rec('x'));x.onreadystatechange=()=>log.push('rs'+x.readyState);x.open('POST','/gated');for(const t of ['abort','loadend'])x.upload.addEventListener(t,rec('u'));x.send('abcd');x.abort()");
    assert_eq!(env.eval("log.join(' ')"), "rs1 rs4 u:abort(0/0,false) u:loadend(0/0,false) x:abort x:loadend");
    server.release.send(()).unwrap();
}

/// Never answers: the decision would be read by the blocked browser thread.
struct SilentInterceptor;
impl pyro_net::Interceptor for SilentInterceptor {
    fn paused(
        &self,
        request: pyro_net::PausedRequest,
        reply: tokio::sync::oneshot::Sender<pyro_net::InterceptDecision>,
    ) {
        std::mem::forget((request, reply));
    }
}

#[test]
fn xhr_sync_blocks_script_and_keeps_the_network_running() {
    let server = Server::new();
    let env = Env::new(&server.url);
    assert_eq!(
        env.eval(
            r#"(()=>{const log=[];globalThis.x=new XMLHttpRequest();globalThis.later=false;setTimeout(()=>later=true);
            x.onreadystatechange=()=>log.push('state'+x.readyState);x.onloadstart=()=>log.push('start');
            x.onprogress=()=>log.push('progress');x.onload=()=>log.push('load:'+later);x.onloadend=()=>log.push('end');
            x.open('GET','/redirect',false);x.send();log.push(later,x.status,x.responseText,x.responseURL.endsWith('/json'));
            return log.join()})()"#
        ),
        r#"state1,state4,load:false,end,false,200,{"answer":42},true"#
    );
    // POST bodies and a following asynchronous request on the same object.
    assert_eq!(
        env.eval(
            "x.onreadystatechange=x.onload=x.onloadend=null;x.open('POST','/echo',false);x.send('sent');x.responseText"
        ),
        "sent"
    );
    env.eval("globalThis.result='pending';x.onload=()=>result=x.response;x.open('GET','/empty');x.send()");
    env.until("result!=='pending'");
    // Not parked on an interceptor only the browser thread could answer.
    env.page.net.borrow().as_ref().unwrap().set_interceptor(Some(std::sync::Arc::new(SilentInterceptor)));
    assert_eq!(env.eval("x.open('GET','/json',false);x.send();x.status"), "200");
    env.page.net.borrow().as_ref().unwrap().set_interceptor(None);
    assert_eq!(env.page.xhr_usage().0, 0);
}

#[test]
fn xhr_sync_failures_throw_and_window_restrictions_apply() {
    let server = Server::new();
    let env = Env::new(&server.url);
    assert_eq!(
        env.eval(
            r#"(()=>{const log=[];const x=new XMLHttpRequest();x.onerror=()=>log.push('error');x.onloadend=()=>log.push('end');
            x.open('GET','http://127.0.0.1:1/refused',false);try{x.send()}catch(e){log.push(e.name,x.readyState,x.status,x.responseText)}
            const name=f=>{try{f();return 'none'}catch(e){return e.name}};
            x.open('GET','/json',false);log.push(name(()=>x.responseType='json'),name(()=>x.timeout=5));
            const y=new XMLHttpRequest();y.responseType='json';log.push(name(()=>y.open('GET','/json',false)));
            const z=new XMLHttpRequest();z.timeout=5;log.push(name(()=>z.open('GET','/json',false)));
            return log.join()})()"#
        ),
        "NetworkError,4,0,,InvalidAccessError,InvalidAccessError,InvalidAccessError,InvalidAccessError"
    );
    assert_eq!(
        env.eval("(()=>{const x=new XMLHttpRequest();x.open('GET','/missing',false);x.send();return [x.status,x.responseText].join()})()"),
        "404,missing"
    );
}

#[test]
fn xhr_document_responses_and_response_xml() {
    let server = Server::new();
    let env = Env::new(&server.url);
    // Synchronous responseXML: XML MIME types parse, others are null.
    assert_eq!(
        env.eval(
            r#"(()=>{const get=(url,mime)=>{const x=new XMLHttpRequest();x.open('GET',url,false);if(mime)x.overrideMimeType(mime);x.send();return x};
            const x=get('/xml');const d=x.responseXML;
            return [d instanceof Document,d===x.responseXML,d.contentType,d.documentElement.namespaceURI,d.documentElement.firstChild.textContent,d.URL.endsWith('/xml'),
              x.response===x.responseText,get('/bad-xml').responseXML,get('/html').responseXML,get('/echo').responseXML,
              get('/json','text/xml').responseXML,get('/latin1-xml').responseXML.documentElement.textContent,
              get('/bad-xml','application/xml;charset=utf-8').responseText].join('|')})()"#
        ),
        "true|true|application/atom+xml|urn:x|t|true|true|||||é|<a><b></a>"
    );
    env.eval(r#"globalThis.result='pending';globalThis.x=new XMLHttpRequest();globalThis.seen=[];
        x.onreadystatechange=()=>{if(x.readyState===3)seen.push(x.response)};
        x.onload=()=>{const d=x.response;result=[d===x.response,d===x.responseXML,d.contentType,d.querySelector('#p').textContent,d.title,
            d.defaultView,typeof ran,d instanceof Document].join('|')};
        x.open('GET','/html');x.responseType='document';x.send();"#);
    env.until("result!=='pending'");
    assert_eq!(env.eval("result"), "true|true|text/html|x|t||undefined|true");
    assert_eq!(env.eval("seen.every(v=>v===null)"), "true");
    assert_eq!(
        env.eval("(()=>{const x=new XMLHttpRequest();x.responseType='json';try{x.responseXML;return 'none'}catch(e){return e.name}})()"),
        "InvalidStateError"
    );
    env.eval(
        r#"globalThis.result='pending';globalThis.x=new XMLHttpRequest();x.onload=()=>result=String(x.response);
        x.open('GET','/bad-xml');x.responseType='document';x.send();"#,
    );
    env.until("result!=='pending'");
    assert_eq!(env.eval("result"), "null");
}
