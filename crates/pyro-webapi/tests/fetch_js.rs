use crate::support;
use pyro_js::RealmOptions;
use pyro_webapi::*;
use std::{rc::Rc, sync::atomic::Ordering, time::Duration};
use support::network::*;

#[test]
fn fetch_json_headers_clone_consumption_and_correlation() {
    let server = Server::new();
    let env = Env::new(&server.url);
    env.page
        .set_request_context(RequestContext { frame_id: Some("frame-7".into()), loader_id: Some("loader-9".into()) });
    let events = env.page.net.borrow().as_ref().unwrap().subscribe();
    env.eval(
        r#"globalThis.result='pending'; fetch('/json').then(async r => {
        if (!(r instanceof Response) || !r.ok || r.status!==200 || r.type!=='basic') throw Error('head');
        if (r.headers!==r.headers || r.headers.has('set-cookie')) throw Error('headers');
        try {r.headers.set('x','y');throw Error('mutable')} catch(e) {if (!(e instanceof TypeError)) throw e;}
        const clone=r.clone(); const data=await r.json();
        if (data.answer!==42 || !r.bodyUsed) throw Error('body');
        try {await r.text();throw Error('reused')} catch(e) {if (!(e instanceof TypeError)) throw e;}
        result=await clone.text();
    }).catch(e=>result='ERROR '+e);"#,
    );
    env.until("result!=='pending'");
    assert_eq!(env.eval("result"), "{\"answer\":42}");
    let mut starts = 0;
    while let Ok(event) = events.try_recv() {
        if let pyro_net::NetEvent::RequestWillBeSent { request, .. } = event {
            starts += 1;
            assert_eq!(request.frame_id.as_deref(), Some("frame-7"));
            assert_eq!(request.loader_id.as_deref(), Some("loader-9"));
            assert_eq!(request.destination, pyro_net::Destination::Fetch);
        }
    }
    assert_eq!(starts, 1);
    assert!(env.posts.load(Ordering::Relaxed) <= 2);
}

#[test]
fn fetch_delivers_headers_before_body_and_abort_preserves_reason() {
    let server = Server::new();
    let env = Env::new(&server.url);
    env.eval("globalThis.ac=new AbortController();globalThis.reason={id:5};globalThis.result='pending';fetch('/gated',{signal:ac.signal}).then(r=>{globalThis.response=r;result='headers';r.text().then(()=>result='unexpected', e=>result=e===reason?'aborted':'wrong reason')},e=>result='ERROR '+e)");
    env.until("result!=='pending'");
    assert_eq!(env.eval("result"), "headers");
    env.eval("ac.abort(reason)");
    assert_eq!(env.eval("result"), "aborted");
    env.finish_network_tasks();
    assert_eq!(env.page.fetch_usage(), (0, 0));
    let _ = server.release.send(());
}

#[test]
fn fetch_abort_reason_reaches_a_stream_read_after_the_response_is_gone() {
    let server = Server::new();
    let env = Env::new(&server.url);
    // The reason reaches back to the signal; only the reader survives the Response wrapper.
    env.eval("globalThis.ac=new AbortController();globalThis.reason={sig:ac.signal};globalThis.result='pending';fetch('/gated',{signal:ac.signal}).then(r=>{globalThis.reader=r.body.getReader();result='headers'},e=>result='ERROR '+e)");
    env.until("result!=='pending'");
    assert_eq!(env.eval("result"), "headers");
    for _ in 0..4 {
        env.engine.collect_garbage_now();
        env.engine.drain_finalized();
        env.page.after_task();
    }
    env.eval("result='pending';reader.read().then(()=>result='unexpected',e=>result=e===reason?'aborted':'wrong reason');ac.abort(reason)");
    assert_eq!(env.eval("result"), "aborted");
    env.finish_network_tasks();
    assert_eq!(env.page.fetch_usage(), (0, 0));
    let _ = server.release.send(());
}

#[test]
fn fetch_abort_reason_comes_from_a_signal_of_another_realm() {
    let server = Server::new();
    let first = Env::new(&server.url);
    let second = Env::with_engine(&server.url, first.engine.clone());
    first.eval("globalThis.ac=new AbortController();globalThis.reason={from:'first'};0");
    {
        let (a, b) = (first.realm.scope().unwrap(), second.realm.scope().unwrap());
        b.global().set("acA", a.global().get("ac").unwrap()).unwrap();
        b.global().set("reasonA", a.global().get("reason").unwrap()).unwrap();
    }
    second.eval("globalThis.result='pending';fetch('/gated',{signal:acA.signal}).then(r=>{globalThis.reader=r.body.getReader();result='headers'},e=>result='ERROR '+e)");
    second.until("result!=='pending'");
    assert_eq!(second.eval("result"), "headers");
    for _ in 0..4 {
        first.engine.collect_garbage_now();
        first.engine.drain_finalized();
    }
    second.eval(
        "result='pending';reader.read().then(()=>result='unexpected',e=>result=e===reasonA?'aborted':'wrong reason')",
    );
    first.eval("ac.abort(reason)");
    assert_eq!(second.eval("result"), "aborted");
    second.finish_network_tasks();
    assert_eq!(second.page.fetch_usage(), (0, 0));
    let _ = server.release.send(());
}

#[test]
fn abort_signal_timeout_reason_identity_and_any() {
    let server = Server::new();
    let env = Env::new(&server.url);
    env.eval("globalThis.t=AbortSignal.timeout(20);globalThis.viaAny=AbortSignal.any([t]);globalThis.result='pending'");
    assert_eq!(env.eval("[t.aborted,viaAny.aborted,t.reason===undefined].join()"), "false,false,true");
    env.until("t.aborted");
    assert_eq!(
        env.eval(
            "const r=t.reason;let thrown;try{t.throwIfAborted()}catch(e){thrown=e};\
             [r instanceof DOMException,r.name,t.reason===r,thrown===r,viaAny.aborted,viaAny.reason===r].join()"
        ),
        "true,TimeoutError,true,true,true,true"
    );
    let _ = server.release.send(());
}

#[test]
fn fetch_with_an_already_aborted_signal_rejects_with_its_reason_without_a_request() {
    let server = Server::new();
    let env = Env::new(&server.url);
    env.eval(
        "globalThis.reason={pre:1};globalThis.result='pending';\
         fetch('/gated',{signal:AbortSignal.abort(reason)}).then(()=>result='unexpected',e=>result=e===reason?'aborted':'wrong reason');\
         globalThis.dflt='pending';\
         fetch('/gated',{signal:AbortSignal.abort()}).then(()=>dflt='unexpected',e=>dflt=e.name)",
    );
    assert_eq!(env.eval("result"), "aborted");
    assert_eq!(env.eval("dflt"), "AbortError");
    assert_eq!(env.page.fetch_usage(), (0, 0));
    let _ = server.release.send(());
}

#[test]
fn fetch_post_bytes_headers_redirects_and_http_errors() {
    let server = Server::new();
    let env = Env::new(&server.url);
    env.eval(r#"globalThis.result='pending'; (async()=>{
        const bytes=new Uint8Array([99,65,66,88]);
        const r=await fetch('/echo',{method:'post',body:bytes.subarray(1,3),headers:[['X-Test','a'],['x-test','b'],['Host','wrong'],['Cookie','wrong=1']]});
        if(await r.text()!=='AB') throw Error('subview');
        const text=await fetch('/echo',{method:'POST',body:'hello'});
        if(await text.text()!=='hello') throw Error('string');
        const blob=await fetch('/echo',{method:'POST',body:new Blob(['blob'],{type:'text/plain'})});
        if(await blob.text()!=='blob') throw Error('blob');
        const missing=await fetch('/missing');
        if(missing.status!==404 || missing.ok || await missing.text()!=='missing') throw Error('404');
        const redirect=await fetch('/redirect');
        if(!redirect.redirected || !redirect.url.endsWith('/json') || (await redirect.json()).answer!==42) throw Error('redirect');
        const manual=await fetch('/redirect',{redirect:'manual'});
        if(manual.type!=='opaqueredirect' || manual.status!==0 || manual.url!=='' || [...manual.headers].length || await manual.text()!=='') throw Error('manual');
        try { await fetch('/redirect',{redirect:'error'}); throw Error('redirect accepted') } catch(e){if(!(e instanceof TypeError))throw e;}
        result='ok';
    })().catch(e=>result='ERROR '+e);"#);
    env.until("result!=='pending'");
    assert_eq!(env.eval("result"), "ok");
    let requests = server.requests.lock().unwrap();
    assert_eq!(requests[0].2, b"AB");
    assert!(requests[0].1.to_ascii_lowercase().contains("x-test: a, b\r\n"));
    assert!(!requests[0].1.contains("wrong"));
    assert!(requests[1].1.to_ascii_lowercase().contains("content-type: text/plain;charset=utf-8"));
    env.finish_network_tasks();
    assert_eq!(env.page.fetch_usage(), (0, 0));
}

#[test]
fn fetch_cors_opaque_guards_and_null_body() {
    let server = Server::new();
    let other = Server::new();
    let env = Env::new(&server.url);
    let remote = other.url.replace("/page", "");
    env.eval(&format!("globalThis.remote='{remote}'"));
    env.eval(r#"globalThis.result='pending'; (async()=>{
        const r=await fetch(remote+'/cors');
        if(r.type!=='cors' || r.headers.get('x-visible')!=='yes' || r.headers.get('x-second')!=='second' || r.headers.has('x-hidden') || r.headers.has('set-cookie') || await r.text()!=='cors') throw Error('CORS filter');
        try {await fetch(remote+'/json');throw Error('CORS accepted')} catch(e){if(!(e instanceof TypeError))throw e;}
        try {await fetch(remote+'/cors',{mode:'same-origin'});throw Error('same-origin accepted')} catch(e){if(!(e instanceof TypeError))throw e;}
        const o=await fetch(remote+'/json',{mode:'no-cors',headers:{'X-Unsafe':'x'}});
        if(o.type!=='opaque' || o.status!==0 || o.url!=='' || o.ok || [...o.headers].length || await o.text()!=='' || o.bodyUsed || (await o.blob()).type!=='') throw Error('opaque');
        const n=await fetch('/empty');
        if(await n.text()!=='' || await n.text()!=='' || n.bodyUsed) throw Error('null body');
        result='ok';
    })().catch(e=>result='ERROR '+e);"#);
    env.until("result!=='pending'");
    assert_eq!(env.eval("result"), "ok");
    assert!(!other.requests.lock().unwrap().last().unwrap().1.to_ascii_lowercase().contains("x-unsafe"));
}

#[test]
fn fetch_binary_json_errors_and_body_budget_release() {
    let server = Server::new();
    let env = Env::new(&server.url);
    env.eval(r#"globalThis.result='pending'; (async()=>{
        const r=await fetch('/json'); globalThis.held=r;
        const a=r.clone(),b=r.clone();
        const bytes=await a.bytes();
        if(!(bytes instanceof Uint8Array) || bytes[0]!==123) throw Error('bytes');
        const buffer=await b.arrayBuffer();
        if(!(buffer instanceof ArrayBuffer) || buffer.byteLength!==13) throw Error('arrayBuffer');
        const blob=await r.blob();
        if(blob.type!=='application/json' || blob.size!==13) throw Error('blob');
        try {await (await fetch('/bad-json')).json();throw Error('JSON accepted')} catch(e){if(!(e instanceof SyntaxError))throw e;}
        result='ok';
    })().catch(e=>result='ERROR '+e);"#);
    env.until("result!=='pending'");
    assert_eq!(env.eval("result"), "ok");
    env.finish_network_tasks();
    assert_eq!(env.page.fetch_usage(), (0, 0));
}

#[test]
fn fetch_abort_before_headers_and_after_completed_body() {
    let server = Server::new();
    let env = Env::new(&server.url);
    env.eval(
        r#"globalThis.result='pending';globalThis.reason={id:42};globalThis.ac=new AbortController();
        const p=fetch('/gated',{signal:ac.signal});ac.abort(reason);
        p.then(()=>result='unexpected',e=>result=e===reason?'aborted':'wrong reason');"#,
    );
    env.until("result!=='pending'");
    assert_eq!(env.eval("result"), "aborted");
    env.finish_network_tasks();
    assert_eq!(env.page.fetch_usage(), (0, 0));
    server.release.send(()).unwrap();
    env.eval(r#"result='pending';globalThis.ac2=new AbortController();fetch('/json',{signal:ac2.signal}).then(r=>{globalThis.unread=r;result='ready'},e=>result='ERROR '+e)"#);
    env.until("result!=='pending'");
    // Headers can precede completion; pump terminal delivery before aborting.
    while env.page.fetch_usage().0 != 0 {
        env.tasks.recv_timeout(Duration::from_secs(5)).unwrap()();
    }
    assert_eq!(env.page.fetch_usage(), (0, 13));
    env.eval("result='pending';ac2.abort(reason);unread.text().then(()=>result='unexpected',e=>result=e===reason?'aborted':'wrong reason')");
    env.until("result!=='pending'");
    assert_eq!(env.eval("result"), "aborted");
    env.finish_network_tasks();
    assert_eq!(env.page.fetch_usage(), (0, 0));
    let count = server.requests.lock().unwrap().len();
    env.eval("result='pending';fetch('/json',{signal:ac2.signal}).then(()=>result='unexpected',e=>result=e===reason?'aborted':'wrong reason')");
    env.until("result!=='pending'");
    assert_eq!(env.eval("result"), "aborted");
    assert_eq!(server.requests.lock().unwrap().len(), count);
}

#[test]
fn fetch_limits_reject_overflow_and_refund_retained_bytes() {
    let server = Server::new();
    let env = Env::new(&server.url);
    assert!(env.page.set_fetch_limits(FetchLimits {
        max_body_bytes: 32,
        max_total_body_bytes: 20,
        ..Default::default()
    }));
    env.eval(r#"globalThis.result='pending';(async()=>{
        globalThis.held=await fetch('/json');
        await held.clone().text();
        try {await (await fetch('/json')).text();throw Error('aggregate accepted')} catch(e){if(!(e instanceof TypeError))throw e;}
        if(await held.text()!=='{"answer":42}') throw Error('original damaged');
        try {await (await fetch('/large')).text();throw Error('large accepted')} catch(e){if(!(e instanceof TypeError))throw e;}
        if((await (await fetch('/json')).json()).answer!==42) throw Error('budget not refunded');
        result='ok';
    })().catch(e=>result='ERROR '+e);"#);
    env.until("result!=='pending'");
    assert_eq!(env.eval("result"), "ok");
    env.finish_network_tasks();
    assert_eq!(env.page.fetch_usage(), (0, 0));
}

#[test]
fn fetch_disposal_cancels_jobs_and_queued_tasks_are_harmless() {
    for whole_page in [false, true] {
        let server = Server::new();
        let env = Env::new(&server.url);
        env.eval("globalThis.result='pending';fetch('/gated').then(r=>{result='headers';r.text().then(()=>result='stale',()=>result='stale')})");
        env.until("result!=='pending'");
        assert_eq!(env.eval("result"), "headers");
        assert_eq!(env.page.fetch_usage().0, 1);
        if whole_page {
            env.page.dispose();
        } else {
            lifecycle::realm_disposed(&env.page, env.realm.id());
        }
        env.finish_network_tasks();
        assert_eq!(env.page.fetch_usage(), (0, 0));
        server.release.send(()).unwrap();
        while let Ok(task) = env.tasks.try_recv() {
            task();
        }
        assert_eq!(env.eval("result"), "headers");
        env.realm.dispose();
        env.engine.collect_garbage_now();
    }
}

#[test]
fn fetch_completed_unread_body_released_after_gc() {
    let server = Server::new();
    let env = Env::new(&server.url);
    env.eval("globalThis.result='pending';fetch('/json').then(r=>{globalThis.response=r;result='ready'})");
    env.until("result!=='pending'");
    while env.page.fetch_usage().0 != 0 {
        env.tasks.recv_timeout(Duration::from_secs(5)).unwrap()();
    }
    assert_eq!(env.page.fetch_usage(), (0, 13));
    env.eval("response=undefined");
    for _ in 0..6 {
        env.eval("Array.from({length:2000}, (_,i)=>({i}))");
        env.engine.collect_garbage_now();
        env.engine.drain_finalized();
        env.page.after_task();
    }
    env.finish_network_tasks();
    assert_eq!(env.page.fetch_usage(), (0, 0));
    assert!(env.page.set_fetch_limits(FetchLimits::default()));
}

#[test]
fn fetch_pending_limit_and_conversion_failures_do_not_start_requests() {
    let server = Server::new();
    let env = Env::new(&server.url);
    assert!(env.page.set_fetch_limits(FetchLimits { max_requests: 1, ..Default::default() }));
    env.eval("globalThis.result='pending';globalThis.ac=new AbortController();fetch('/gated',{signal:ac.signal}).then(r=>{globalThis.response=r;result='headers'})");
    env.until("result!=='pending'");
    let count = server.requests.lock().unwrap().len();
    env.eval(r#"result='pending'; (async()=>{
        for (const init of [{}, {method:'TRACE'}, {body:'x'}, {headers:{'bad name':'x'}}, {signal:{}}, {mode:'wrong'}, {get method(){throw new TypeError('getter')}}]) {
            try {await fetch('/json',init);throw Error('accepted')} catch(e){if(!(e instanceof TypeError))throw e;}
        }
        result='ok';
    })().catch(e=>result='ERROR '+e);"#);
    env.until("result!=='pending'");
    assert_eq!(env.eval("result"), "ok");
    assert_eq!(server.requests.lock().unwrap().len(), count);
    env.eval("ac.abort()");
    env.finish_network_tasks();
    assert_eq!(env.page.fetch_usage(), (0, 0));
    server.release.send(()).unwrap();
}

#[test]
fn fetch_null_body_remains_readable_after_abort() {
    let server = Server::new();
    let env = Env::new(&server.url);
    env.eval(r#"globalThis.result='pending';globalThis.ac=new AbortController();fetch('/empty',{signal:ac.signal}).then(async r=>{
        ac.abort();
        if(await r.text()!=='' || r.bodyUsed || await r.text()!=='') throw Error('null body');
        result='ok';
    }).catch(e=>result='ERROR '+e);"#);
    env.until("result!=='pending'");
    assert_eq!(env.eval("result"), "ok");
}

#[test]
fn response_borrowed_methods_work_across_realms_without_self_roots() {
    let server = Server::new();
    let env = Env::new(&server.url);
    env.eval("globalThis.result='pending';fetch('/json').then(r=>{globalThis.response=r;result='ready'})");
    env.until("result!=='pending'");
    while env.page.fetch_usage().0 != 0 {
        env.tasks.recv_timeout(Duration::from_secs(5)).unwrap()();
    }
    let class = env.engine.register::<Window>();
    let other = env.engine.create_realm(RealmOptions { global_class: Some(class), ..Default::default() }).unwrap();
    let key = env.page.create_window_target(env.page.document());
    other
        .set_global_data::<Window>(WindowData {
            page: env.page.clone(),
            target: EventTargetData::window(&env.page, key),
        })
        .unwrap();
    {
        let s = other.scope().unwrap();
        install(&s, env.page.clone()).unwrap();
        let main = env.realm.scope().unwrap();
        s.global().set("foreign", main.global().get("response").unwrap()).unwrap();
        assert_eq!(
            s.eval(
                "Object.getOwnPropertyDescriptor(Response.prototype,'headers').get.call(foreign)===foreign.headers",
                None,
                1
            )
            .unwrap()
            .to_string()
            .unwrap(),
            "true"
        );
        s.eval(
            "globalThis.result='pending';Response.prototype.clone.call(foreign).json().then(v=>result=v.answer)",
            None,
            1,
        )
        .unwrap();
        assert_eq!(s.eval("result", None, 1).unwrap().to_string().unwrap(), "42");
    }
    lifecycle::realm_disposed(&env.page, other.id());
    other.dispose();
    env.eval("result='pending';response.text().then(v=>result=v)");
    env.until("result!=='pending'");
    assert_eq!(env.eval("result"), "{\"answer\":42}");
    env.finish_network_tasks();
    assert_eq!(env.page.fetch_usage(), (0, 0));
}

#[test]
fn fetch_disposal_before_queued_headers_preserves_shared_context_network() {
    let server = Server::new();
    let first = Env::new(&server.url);
    let second = Env::with_engine(&server.url, first.engine.clone());
    *second.page.net.borrow_mut() = first.page.net.borrow().clone();
    first.eval("globalThis.result='pending';fetch('/gated').then(()=>result='stale',()=>result='stale')");
    let stale = first.tasks.recv_timeout(Duration::from_secs(5)).unwrap();
    first.page.dispose();
    stale();
    assert_eq!(first.eval("result"), "pending");
    assert_eq!(first.page.fetch_usage(), (0, 0));
    let weak = Rc::downgrade(&first.page);
    drop(first);
    for _ in 0..6 {
        second.engine.collect_garbage_now();
        second.engine.drain_finalized();
    }
    assert!(weak.upgrade().is_none(), "queued fetch state retained disposed page");
    server.release.send(()).unwrap();
    second.eval(
        "globalThis.result='pending';fetch('/json').then(r=>r.json()).then(v=>result=v.answer,e=>result='ERROR '+e)",
    );
    second.until("result!=='pending'");
    assert_eq!(second.eval("result"), "42");
}

#[test]
fn fetch_head_abort_before_headers_rejects_without_panicking() {
    let server = Server::new();
    let env = Env::new(&server.url);
    env.eval("globalThis.result='pending';globalThis.reason={head:true};globalThis.ac=new AbortController();fetch('/gated',{method:'HEAD',signal:ac.signal}).then(()=>result='unexpected',e=>result=e===reason?'aborted':'wrong reason');ac.abort(reason)");
    env.until("result!=='pending'");
    assert_eq!(env.eval("result"), "aborted");
    assert_eq!(env.page.fetch_usage(), (0, 0));
    server.release.send(()).unwrap();
}

#[test]
fn fetch_abort_before_queued_null_headers_rejects_with_original_reason() {
    let server = Server::new();
    let env = Env::new(&server.url);
    env.eval("globalThis.result='pending';globalThis.reason={empty:true};globalThis.ac=new AbortController();fetch('/empty',{signal:ac.signal}).then(()=>result='unexpected',e=>result=e===reason?'aborted':'wrong reason')");
    let queued_headers = env.tasks.recv_timeout(Duration::from_secs(5)).unwrap();
    env.eval("ac.abort(reason)");
    assert_eq!(env.eval("result"), "aborted");
    queued_headers();
    assert_eq!(env.eval("result"), "aborted");
    assert_eq!(env.page.fetch_usage(), (0, 0));
}

#[test]
fn response_constructor_statics_and_body_semantics() {
    let server = Server::new();
    let env = Env::new(&server.url);
    env.eval(
        r#"globalThis.result='pending'; (async()=>{
        const check=(c,m)=>{if(!c) throw Error(m)};
        // TOS redirect chunk pattern: new Response(null,{status,headers}) then an expando.
        const h=new Headers({'X-Redirect-To':'/platform'});
        const n=new Response(null,{status:307,headers:h}); n.options={to:'/x'};
        check(n instanceof Response && n.status===307 && !n.ok && n.type==='default' && n.url==='' && !n.redirected,'null');
        check(n.headers.get('x-redirect-to')==='/platform' && n.headers!==h && n.options.to==='/x' && n.body===null,'null headers');
        check(await n.text()==='' && !n.bodyUsed,'null body read');
        const r=new Response('héllo',{status:201,statusText:'Created',headers:[['Set-Cookie','a=1'],['X-A','1']]});
        check(r.ok && r.status===201 && r.statusText==='Created','status');
        check(r.headers.get('content-type')==='text/plain;charset=UTF-8' && !r.headers.has('set-cookie'),'ct');
        r.headers.set('X-B','2'); check(r.headers.get('x-b')==='2','mutable headers');
        const c=r.clone(); c.headers.set('X-C','3'); check(!r.headers.has('x-c'),'clone headers independent');
        check(await r.text()==='héllo' && r.bodyUsed,'text');
        let reused=false; try{await r.text()}catch(e){reused=e instanceof TypeError} check(reused,'reuse');
        let cloneUsed=false; try{r.clone()}catch(e){cloneUsed=e instanceof TypeError} check(cloneUsed,'clone after use');
        check(new TextDecoder().decode(await c.arrayBuffer())==='héllo','clone body');
        check((await new Response(new Uint8Array([1,2,3])).bytes()).join()==='1,2,3','bytes');
        check((await new Response(new URLSearchParams({a:'b c'})).text())==='a=b+c','params');
        const blob=await new Response(new Blob(['x'],{type:'text/x'})).blob(); check(blob.type==='text/x' && blob.size===1,'blob');
        check(new Response().status===200 && new Response().statusText==='','defaults');
        const j=Response.json({k:[1]},{status:202}); check(j.status===202 && j.headers.get('content-type')==='application/json' && (await j.json()).k[0]===1,'json');
        const e=Response.error(); check(e.type==='error' && e.status===0 && !e.ok,'error');
        let imm=false; try{e.headers.set('a','b')}catch(x){imm=x instanceof TypeError} check(imm,'error headers immutable');
        const rd=Response.redirect('/next',301); check(rd.status===301 && rd.headers.get('location')===new URL('/next',document.URL).href,'redirect');
        const throws=(f,T)=>{try{f();return false}catch(x){return x instanceof T}};
        check(throws(()=>new Response('',{status:199}),RangeError) && throws(()=>new Response('',{status:600}),RangeError),'status range');
        check(throws(()=>new Response('x',{status:204}),TypeError) && throws(()=>new Response('',{statusText:'a\nb'}),TypeError),'null body status / statusText');
        check(throws(()=>Response.redirect('/x',200),RangeError),'redirect status');
        check(throws(()=>Response.json(undefined),TypeError),'json undefined');
        result='ok';
    })().catch(e=>result='ERROR '+e.message);"#,
    );
    env.until("result!=='pending'");
    assert_eq!(env.eval("result"), "ok");
}

#[test]
fn form_data_entries_iteration_and_multipart_bodies() {
    let server = Server::new();
    let env = Env::new(&server.url);
    env.eval(
        r#"globalThis.result='pending'; (async()=>{
        const check=(c,m)=>{if(!c) throw Error(m)};
        const fd=new FormData();
        // TOS native REST client checks `data instanceof FormData` for every body.
        check(!({a:1} instanceof FormData) && fd instanceof FormData,'instanceof');
        fd.append('a','1'); fd.append('a',2); fd.append('b','x');
        check(fd.get('a')==='1' && fd.getAll('a').join()==='1,2' && fd.has('b') && fd.get('zz')===null,'get');
        fd.set('a','3'); check(fd.getAll('a').join()==='3' && [...fd.keys()].join()==='a,b','set keeps position');
        const blob=new Blob(['hello'],{type:'text/plain'});
        fd.append('f',blob); fd.append('g',blob,'g.txt');
        const f=fd.get('f'); check(f instanceof File && f.name==='blob' && fd.get('f')===f,'blob->File identity');
        check(fd.get('g').name==='g.txt','filename');
        const named=new File(['z'],'z.bin'); fd.append('h',named); check(fd.get('h')===named,'file identity');
        const seen=[]; fd.forEach((v,k)=>seen.push(k)); check(seen.join()==='a,b,f,g,h','forEach');
        check([...fd].map(p=>p[0]).join()==='a,b,f,g,h' && [...fd.entries()][0][1]==='3','iterator');
        fd.delete('h'); check(!fd.has('h'),'delete');
        const r=await fetch('/echo',{method:'POST',body:fd});
        const text=await r.text();
        check(/name="a"\r\n\r\n3\r\n/.test(text) && /name="f"; filename="blob"\r\nContent-Type: text\/plain\r\n\r\nhello\r\n/.test(text),'multipart body');
        const x=new XMLHttpRequest(); x.open('POST','/echo'); await new Promise((res,rej)=>{x.onload=res;x.onerror=rej;x.send(fd)});
        check(/name="g"; filename="g.txt"/.test(x.responseText),'xhr multipart');
        let threw=false; try{ new FormData({}) }catch(e){ threw=e instanceof TypeError } check(threw,'non-form arg');
        result='ok';
    })().catch(e=>result='ERROR '+e.message);"#,
    );
    env.until("result!=='pending'");
    assert_eq!(env.eval("result"), "ok");
}

fn assert_fetch_owner_delivery(path: &str, expected: &str) {
    let server = Server::new();
    let env = Env::new(&server.url);
    env.eval(&format!("globalThis.response=null;globalThis.result='pending';fetch('{path}').then(r=>response=r)"));
    env.until("response!==null");
    server.release.send(()).unwrap();
    let completion = env.tasks.recv_timeout(Duration::from_secs(5)).expect("transport completion queued");
    env.eval("response.text().then(v=>result=v,()=>result='error')");
    assert_eq!(env.eval("result"), "pending", "transport completion must not escape before its owner task");
    completion();
    env.until("result!=='pending'");
    assert_eq!(env.eval("result"), expected);
}

#[test]
fn fetch_body_waits_for_owner_delivery() {
    assert_fetch_owner_delivery("/gated", "abcdef");
}

#[test]
fn fetch_error_waits_for_owner_delivery() {
    assert_fetch_owner_delivery("/gated-failure", "error");
}

#[test]
fn fetch_delivery_frontier_reaches_the_hook_before_dependent_js() {
    let server = Server::new();
    let env = Env::new(&server.url);
    env.eval("globalThis.stage='pending';fetch('/gated').then(r=>{__log('js response');stage='response';return r.text()}).then(t=>{__log('js text '+t);stage='done'})");
    env.until("stage==='response'");
    server.release.send(()).unwrap();
    env.until("stage==='done'");
    let frontiers = env.frontiers();
    assert_eq!(frontiers.first(), Some(&(true, 0, false)), "headers are delivered before the body: {frontiers:?}");
    assert_eq!(frontiers.last(), Some(&(true, 6, true)));
    let first = env.log_index(|l| l.starts_with("delivery ")).unwrap();
    let terminal = env.log_index(|l| l.starts_with("delivery ") && l.ends_with("t=true")).unwrap();
    assert!(first < env.log_index(|l| l == "js response").unwrap(), "{:?}", env.log.borrow());
    assert!(terminal < env.log_index(|l| l == "js text abcdef").unwrap(), "{:?}", env.log.borrow());
}

#[test]
fn fetch_null_body_frontier_is_terminal_without_bytes() {
    let server = Server::new();
    let env = Env::new(&server.url);
    env.eval("globalThis.stage='pending';fetch('/empty').then(r=>r.text()).then(t=>{__log('js text');stage='done'})");
    env.until("stage==='done'");
    // A null body is readable immediately at headers; it need not wait for
    // the I/O thread's separate Finished notification. Drain owner tasks
    // before asserting that the real transport terminal reached the hook.
    env.finish_network_tasks();
    assert_eq!(env.frontiers().last(), Some(&(true, 0, true)));
}

#[test]
fn fetch_local_abort_reports_no_transport_terminal() {
    let server = Server::new();
    let env = Env::new(&server.url);
    env.eval("globalThis.ac=new AbortController();globalThis.stage='pending';fetch('/gated',{signal:ac.signal}).then(r=>{stage='response';return r.text()}).then(()=>stage='text',e=>stage='error:'+e.name)");
    env.until("stage==='response'");
    env.eval("ac.abort()");
    env.until("stage!=='response'");
    assert_eq!(env.eval("stage"), "error:AbortError");
    server.release.send(()).unwrap();
    env.drain_for(Duration::from_millis(300));
    assert!(env.frontiers().iter().all(|f| !f.2), "a local abort is not a transport terminal: {:?}", env.log.borrow());
}

#[test]
fn fetch_disposal_stops_later_frontiers() {
    // Frontiers already reported stay; none follow disposal.
    let server = Server::new();
    let env = Env::new(&server.url);
    env.eval("globalThis.stage='pending';fetch('/gated').then(r=>{stage='response';return r.text()})");
    env.until("stage==='response'");
    let before = env.log.borrow().len();
    env.page.dispose();
    server.release.send(()).unwrap();
    env.drain_for(Duration::from_millis(300));
    assert_eq!(env.log.borrow().len(), before, "no hook after disposal: {:?}", env.log.borrow());
}

#[test]
fn page_extra_http_headers_are_page_scoped_replaced_and_cleared() {
    let server = Server::new();
    let first = Env::new(&server.url);
    let second = Env::with_engine(&server.url, first.engine.clone());
    *second.page.net.borrow_mut() = first.page.net.borrow().clone();
    let headers_of = |path: &str| -> String {
        let requests = server.requests.lock().unwrap();
        let (_, headers, _) = requests.iter().rev().find(|(line, _, _)| line.contains(path)).expect(path);
        headers.to_ascii_lowercase()
    };
    let fetch = |env: &Env, path: &str| {
        env.eval(&format!("globalThis.result='pending';fetch('{path}').then(r=>r.text()).then(()=>result='done')"));
        env.until("result==='done'");
    };
    let xhr = |env: &Env, path: &str| {
        env.eval(&format!("globalThis.result='pending';globalThis.x=new XMLHttpRequest();x.onload=()=>result='done';x.open('GET','{path}');x.send()"));
        env.until("result==='done'");
    };
    let mut map = pyro_net::http::HeaderMap::new();
    map.insert("x-page", "a".parse().unwrap());
    map.insert("cache-control", "no-cache".parse().unwrap());
    first
        .page
        .set_user_agent_override(Some(UserAgentOverride {
            user_agent: "page-a-agent".into(),
            accept_language: None,
            platform: None,
            metadata: None,
        }))
        .unwrap();
    first.page.set_extra_http_headers(map);

    fetch(&first, "/json?a1");
    let a1 = headers_of("/json?a1");
    assert!(a1.contains("x-page: a") && a1.contains("cache-control: no-cache"), "{a1}");
    assert!(a1.contains("user-agent: page-a-agent"), "the UA override is kept alongside: {a1}");
    xhr(&first, "/json?a2");
    assert!(headers_of("/json?a2").contains("x-page: a"));

    // A sibling page on the same context network client never sees them.
    fetch(&second, "/json?b1");
    xhr(&second, "/json?b2");
    for path in ["/json?b1", "/json?b2"] {
        let h = headers_of(path);
        assert!(!h.contains("x-page") && !h.contains("cache-control"), "{path}: {h}");
    }

    // Replace: the whole set changes. A later UA override keeps the extra headers.
    let mut map = pyro_net::http::HeaderMap::new();
    map.insert("x-other", "b".parse().unwrap());
    first.page.set_extra_http_headers(map);
    first.page.set_user_agent_override(None).unwrap();
    fetch(&first, "/json?a3");
    let a3 = headers_of("/json?a3");
    assert!(a3.contains("x-other: b") && !a3.contains("x-page") && !a3.contains("cache-control"), "{a3}");
    assert!(!a3.contains("page-a-agent"), "{a3}");

    // Clear.
    first.page.set_extra_http_headers(pyro_net::http::HeaderMap::new());
    assert!(first.page.extra_http_headers().is_empty());
    fetch(&first, "/json?a4");
    assert!(!headers_of("/json?a4").contains("x-other"));
}

#[test]
fn fetch_cache_modes_use_the_context_http_cache() {
    let server = Server::new();
    let env = Env::new(&server.url);
    let hits = || server.requests.lock().unwrap().iter().filter(|(line, _, _)| line.contains("/cacheable")).count();
    let fetch = |init: &str| {
        env.eval(&format!(
            "globalThis.result='pending';fetch('/cacheable',{init}).then(r=>r.text().then(t=>result=r.status+':'+t),e=>result=e.name)"
        ));
        env.until("result!=='pending'");
        env.eval("result")
    };
    assert_eq!(fetch("{}"), "200:cached");
    assert_eq!(fetch("{}"), "200:cached");
    assert_eq!(hits(), 1, "the second load is a fresh cache hit");
    assert_eq!(fetch("{cache:'no-store'}"), "200:cached");
    assert_eq!(hits(), 2);
    let last = server.requests.lock().unwrap().last().unwrap().1.to_ascii_lowercase();
    assert!(last.contains("pragma: no-cache") && last.contains("cache-control: no-cache"), "{last}");
    assert_eq!(fetch("{cache:'reload'}"), "200:cached");
    assert_eq!(fetch("{cache:'force-cache'}"), "200:cached");
    assert_eq!(fetch("{cache:'only-if-cached',mode:'same-origin'}"), "200:cached");
    assert_eq!(hits(), 3, "reload goes to the network; force-cache and only-if-cached do not");
    env.eval("globalThis.errors=null;Promise.allSettled([{cache:'bogus'},{cache:'only-if-cached'}].map(i=>fetch('/cacheable',i))).then(r=>errors=r.map(x=>x.status+':'+(x.reason&&x.reason.name)).join())");
    env.until("errors!==null");
    assert_eq!(env.eval("errors"), "rejected:TypeError,rejected:TypeError");
    assert_eq!(hits(), 3, "rejected inits never reach the network");
}

#[test]
fn page_cache_disabled_bypasses_only_that_page() {
    let server = Server::new();
    let first = Env::new(&server.url);
    let second = Env::with_engine(&server.url, first.engine.clone());
    *second.page.net.borrow_mut() = first.page.net.borrow().clone();
    let hits = || server.requests.lock().unwrap().iter().filter(|(line, _, _)| line.contains("/cacheable")).count();
    let last_headers = || server.requests.lock().unwrap().last().unwrap().1.to_ascii_lowercase();
    let load = |env: &Env, xhr: bool| {
        if xhr {
            env.eval("globalThis.result='pending';globalThis.x=new XMLHttpRequest();x.onload=()=>result='done';x.open('GET','/cacheable');x.send()");
        } else {
            env.eval("globalThis.result='pending';fetch('/cacheable').then(r=>r.text()).then(()=>result='done')");
        }
        env.until("result==='done'");
    };
    load(&second, false);
    assert_eq!(hits(), 1);
    first.page.set_cache_disabled(true);
    assert!(first.page.cache_disabled());
    load(&first, false);
    assert_eq!(hits(), 2, "the disabled page bypasses the context cache");
    assert!(last_headers().contains("pragma: no-cache") && last_headers().contains("cache-control: no-cache"));
    load(&first, true);
    assert_eq!(hits(), 3, "XHR bypasses too");
    load(&second, false);
    load(&second, true);
    assert_eq!(hits(), 3, "the sibling page keeps hitting the shared context cache");
    first.page.set_cache_disabled(false);
    load(&first, false);
    assert_eq!(hits(), 3, "re-enabled: the page hits again");
}

#[test]
fn inherited_document_base_is_used_by_fetch_and_xhr_without_replacing_url() {
    let server = Server::new();
    let env = Env::with_engine_origin(
        &server.url,
        Rc::new(pyro_js::Engine::new().unwrap()),
        Some(pyro_net::Url::parse(&server.url).unwrap().origin().ascii_serialization()),
    );
    let doc = env.page.document();
    {
        let mut dom = env.page.dom.borrow_mut();
        dom.set_document_url(doc, "about:srcdoc").unwrap();
        dom.set_document_base_url(doc, Some(&server.url)).unwrap();
    }
    env.eval("globalThis.result='pending';fetch('json').then(r=>r.text()).then(v=>result=v,e=>result=String(e))");
    env.until("result!=='pending'");
    assert_eq!(env.eval("result"), "{\"answer\":42}");
    env.eval("result='pending';const x=new XMLHttpRequest();x.onload=()=>result=x.responseText;x.onerror=()=>result='error';x.open('GET','json');x.send()");
    env.until("result!=='pending'");
    assert_eq!(env.eval("result"), "{\"answer\":42}");
    assert_eq!(env.eval("document.URL"), "about:srcdoc");
}

#[test]
fn fetch_body_stream_delivers_chunks_as_they_arrive() {
    // TOS chat reads /chat/events (SSE) with response.body.getReader(); each
    // frame must resolve a read before the response completes.
    let server = Server::new();
    let env = Env::new(&server.url);
    env.eval(
        r#"globalThis.frames=[]; globalThis.state='pending';
        (async()=>{
          const r=await fetch('/sse');
          const body=r.body;
          if (body!==r.body || body.locked || r.bodyUsed) throw Error('fresh stream');
          const reader=body.getReader();
          if (!body.locked || r.bodyUsed) throw Error('locked');
          try { body.getReader(); throw Error('second reader') } catch(e) { if (!(e instanceof TypeError)) throw e; }
          try { await r.text(); throw Error('text after reader') } catch(e) { if (!(e instanceof TypeError)) throw e; }
          const decoder=new TextDecoder(); let buffer='';
          while (true) {
            const {value, done}=await reader.read();
            if (!done && !(value instanceof Uint8Array)) throw Error('chunk type');
            buffer+=decoder.decode(value,{stream:!done});
            const parts=buffer.split('\n\n'); buffer=parts.pop();
            for (const part of parts) frames.push(part.match(/^id: (.*)$/m)[1]+'='+part.match(/^data: (.*)$/m)[1]);
            state=frames.join(' ');
            if (done) break;
          }
          state='done '+frames.join(' ');
        })().catch(e=>state='ERROR '+e);"#,
    );
    env.until("state!=='pending'");
    assert_eq!(env.eval("state"), "1={\"n\":1}");
    let _ = server.release.send(());
    env.until("state!=='1={\"n\":1}'");
    assert_eq!(env.eval("state"), "1={\"n\":1} 2={\"n\":2}");
    let _ = server.release.send(());
    env.until("state.startsWith('done') || state.startsWith('ERROR')");
    assert_eq!(env.eval("state"), "done 1={\"n\":1} 2={\"n\":2}");
    env.finish_network_tasks();
    assert_eq!(env.page.fetch_usage(), (0, 0), "streamed bytes and the job are released");
}

#[test]
fn fetch_body_stream_cancel_release_and_script_bodies() {
    let server = Server::new();
    let env = Env::new(&server.url);
    env.eval(
        r#"globalThis.state='pending';
        (async()=>{
          const out=[];
          // Script-created body: one chunk, then done.
          const s=new Response('abc').body.getReader();
          const first=await s.read(); const second=await s.read();
          out.push(new TextDecoder().decode(first.value)+':'+first.done+':'+second.done+':'+second.value);
          // Null-body responses have no stream.
          out.push(String((await fetch('/empty')).body));
          // releaseLock unlocks; a new reader continues the same stream.
          const r=await fetch('/json'); const a=r.body.getReader(); a.releaseLock();
          out.push(r.body.locked+':'+(await r.body.getReader().read()).value.length);
          try { await a.read(); out.push('released read') } catch(e) { out.push(e instanceof TypeError ? 'released' : 'other') }
          // cancel ends the stream: pending and later reads are done.
          const c=await fetch('/sse'); const reader=c.body.getReader(); await reader.read();
          const pending=reader.read(); await reader.cancel();
          out.push((await pending).done+':'+(await reader.read()).done);
          out.push(c.body instanceof ReadableStream);
          state=out.join('|');
        })().catch(e=>state='ERROR '+e);"#,
    );
    env.until("state!=='pending'");
    assert_eq!(env.eval("state"), "abc:false:true:undefined|null|false:13|released|true:true|true");
    env.finish_network_tasks();
    assert_eq!(env.page.fetch_usage(), (0, 0));
}

#[test]
fn streams_are_lazy_globals_and_compose() {
    let server = Server::new();
    let env = Env::new(&server.url);
    // Names exist without making the classes; assignment replaces them.
    assert_eq!(env.eval("typeof Object.getOwnPropertyDescriptor(globalThis,'ReadableStream').get"), "function");
    assert_eq!(env.eval("CountQueuingStrategy=1; typeof CountQueuingStrategy"), "number");
    env.eval(
        r#"globalThis.state='pending';
        (async()=>{
          const out=[];
          const descriptor=Object.getOwnPropertyDescriptor(globalThis,'ReadableStream');
          out.push(typeof ReadableStream);
          out.push('value' in Object.getOwnPropertyDescriptor(globalThis,'ReadableStream') && !!descriptor.get);
          // start/pull/cancel, default reader, desiredSize.
          let pulls=0, canceled;
          const s=new ReadableStream({start(c){c.enqueue('a')},pull(c){if(++pulls==2)c.close();else c.enqueue('b')},cancel(r){canceled=r}});
          const reader=s.getReader();
          out.push([(await reader.read()).value,(await reader.read()).value,(await reader.read()).done].join());
          await reader.closed;
          reader.releaseLock();
          // tee, async iteration, cancel reason.
          const [t1,t2]=new ReadableStream({start(c){c.enqueue(1);c.enqueue(2);c.close()}}).tee();
          const seen=[]; for await (const v of t1) seen.push(v);
          out.push(seen.join()+':'+(await t2.getReader().read()).value);
          const cs=new ReadableStream({cancel(r){canceled=r}}); await cs.cancel('why'); out.push(canceled);
          // pipeThrough/TransformStream/WritableStream.
          const collected=[];
          await new ReadableStream({start(c){c.enqueue('x');c.enqueue('y');c.close()}})
            .pipeThrough(new TransformStream({transform(ch,c){c.enqueue(ch+ch)}}))
            .pipeTo(new WritableStream({write(ch){collected.push(ch)}}));
          out.push(collected.join());
          // Text streams hold split characters across chunks.
          const euro=new TextEncoder().encode('\u20ac');
          const dec=new ReadableStream({start(c){c.enqueue(euro.slice(0,1));c.enqueue(euro.slice(1));c.close()}})
            .pipeThrough(new TextDecoderStream());
          out.push((await dec.getReader().read()).value);
          // new Response(stream), body identity, clone, Blob.stream, BYOB.
          const body=new ReadableStream({start(c){c.enqueue(new TextEncoder().encode('hi'));c.close()}});
          const made=new Response(body,{headers:{'content-type':'text/x'}});
          out.push(made.body===body);
          const copy=made.clone();
          out.push(await made.text()+await copy.text()+made.bodyUsed);
          const blobReader=new Blob(['blob']).stream().getReader({mode:'byob'});
          const {value}=await blobReader.read(new Uint8Array(2));
          out.push(value.length+':'+String.fromCharCode(value[0]));
          // A network response is a byte stream too.
          const net=await fetch('/json');
          let text=''; for await (const c of net.body.pipeThrough(new TransformStream())) text+=new TextDecoder().decode(c);
          out.push(text.length+':'+net.bodyUsed);
          // A stream as a request body is sent once it ended; duplex is required.
          const send=()=>new ReadableStream({start(c){c.enqueue(new TextEncoder().encode('up'));c.close()}});
          out.push(await fetch('/json',{method:'POST',body:send()}).then(()=>'sent',e=>e.name));
          out.push(await fetch('/json',{method:'POST',body:send(),duplex:'half'}).then(r=>r.status));
          state=out.join('|');
        })().catch(e=>state='ERROR '+e+' '+e.stack);"#,
    );
    env.until("state!=='pending'");
    assert_eq!(
        env.eval("state"),
        "function|true|a,b,true|1,2:1|why|xx,yy|\u{20ac}|true|hihitrue|2:b|13:true|TypeError|200"
    );
    env.finish_network_tasks();
}

#[test]
fn text_decoder_streams_split_characters() {
    let server = Server::new();
    let env = Env::new(&server.url);
    assert_eq!(
        env.eval(
            "const b=new Uint8Array([0xef,0xbb,0xbf,0xe2,0x82,0xac]);const d=new TextDecoder();\
             [d.decode(b.slice(0,4),{stream:true}),d.decode(b.slice(4),{stream:true}),d.decode()].join('|')+\
             '|'+new TextDecoder().decode(b.slice(3,4)).length"
        ),
        "|\u{20ac}||1"
    );
}

#[test]
fn fetch_body_stream_abort_rejects_pending_read_with_reason() {
    // Own server: the test server serves one connection at a time.
    let server = Server::new();
    let env = Env::new(&server.url);
    env.eval(
        r#"globalThis.state='pending';
        (async()=>{
          const ac=new AbortController(); const reason={why:'stop'};
          const reader=(await fetch('/sse',{signal:ac.signal})).body.getReader(); await reader.read();
          const waiting=reader.read(); ac.abort(reason);
          state=await waiting.then(()=>'resolved', e=>e===reason?'aborted':'other '+e);
        })().catch(e=>state='ERROR '+e);"#,
    );
    env.until("state!=='pending'");
    assert_eq!(env.eval("state"), "aborted");
    let _ = server.release.send(());
    env.finish_network_tasks();
    assert_eq!(env.page.fetch_usage(), (0, 0));
}

/// The recorded request whose first line contains `needle`, waiting for the
/// server to see it.
fn recorded(server: &Server, needle: &str) -> (String, String, Vec<u8>) {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(r) = server.requests.lock().unwrap().iter().find(|(first, _, _)| first.contains(needle)) {
            return r.clone();
        }
        assert!(std::time::Instant::now() < deadline, "no request {needle}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn header<'a>(headers: &'a str, name: &str) -> Option<&'a str> {
    headers.lines().find_map(|l| {
        let (n, v) = l.split_once(':')?;
        n.trim().eq_ignore_ascii_case(name).then(|| v.trim())
    })
}

/// keepalive, referrer, referrerPolicy and integrity are accepted (they used
/// to reject) and honoured: the Referer follows the referrer and policy.
#[test]
fn fetch_referrer_policy_keepalive_and_init_members() {
    let server = Server::new();
    let env = Env::new(&server.url);
    let origin = server.url.trim_end_matches("/page");
    env.eval(
        "globalThis.result='pending';Promise.all([\
         fetch('/json?default'),\
         fetch('/json?origin',{referrerPolicy:'origin',keepalive:true}),\
         fetch('/json?none',{referrer:''}),\
         fetch('/json?custom',{referrer:'/from/here?x#y',referrerPolicy:'unsafe-url'}),\
         fetch(new Request('/json?request',{referrerPolicy:'no-referrer'})),\
         fetch('/json?other',{referrer:'https://elsewhere.test/'}),\
        ]).then(rs=>result=rs.map(r=>r.status).join(),e=>result='ERROR '+e)",
    );
    env.until("result!=='pending'");
    assert_eq!(env.eval("result"), "200,200,200,200,200,200");
    let referer = |q: &str| header(&recorded(&server, q).1, "referer").map(str::to_owned);
    assert_eq!(referer("?default").as_deref(), Some(server.url.as_str()));
    assert_eq!(referer("?origin"), Some(format!("{origin}/")));
    assert_eq!(referer("?none"), None);
    assert_eq!(referer("?custom"), Some(format!("{origin}/from/here?x")));
    assert_eq!(referer("?request"), None);
    // A cross-origin referrer falls back to the document ("client").
    assert_eq!(referer("?other").as_deref(), Some(server.url.as_str()));
    assert_eq!(header(&recorded(&server, "?default").1, "sec-fetch-mode"), Some("cors"));
    assert_eq!(header(&recorded(&server, "?default").1, "sec-fetch-site"), Some("same-origin"));
    assert_eq!(env.eval("String(new Request('/x',{referrer:'https://elsewhere.test/'}).referrer)"), "about:client");
    env.eval("globalThis.bad='pending';fetch('/json',{referrerPolicy:'bogus'}).then(()=>bad='ok',e=>bad=e.name)");
    env.until("bad!=='pending'");
    assert_eq!(env.eval("bad"), "TypeError");
    // Keepalive bodies share a 64 KiB quota.
    env.eval("globalThis.big='pending';fetch('/echo',{method:'POST',body:'x'.repeat(70000),keepalive:true}).then(()=>big='ok',e=>big=e.name)");
    env.until("big!=='pending'");
    assert_eq!(env.eval("big"), "TypeError");
    let _ = server.release.send(());
}

/// The document's `<meta name=referrer>` sets the policy of its requests.
#[test]
fn fetch_uses_the_document_meta_referrer_policy() {
    let server = Server::new();
    let env = Env::new(&server.url);
    env.eval(
        "const m=document.createElement('meta');m.name='referrer';m.content='no-referrer';document.head.append(m);\
         globalThis.result='pending';fetch('/json?meta').then(r=>result=r.status,e=>result='ERROR '+e)",
    );
    env.until("result!=='pending'");
    assert_eq!(header(&recorded(&server, "?meta").1, "referer"), None);
    assert_eq!(env.page.referrer_policy(), pyro_net::ReferrerPolicy::NoReferrer);
    let _ = server.release.send(());
}

/// Subresource Integrity: a matching digest delivers the response, a
/// mismatch or an opaque response rejects with a TypeError.
#[test]
fn fetch_integrity_checks_the_whole_body() {
    use base64::Engine as _;
    use sha2::Digest as _;
    let server = Server::new();
    let env = Env::new(&server.url);
    let good = format!(
        "sha256-{}",
        base64::engine::general_purpose::STANDARD.encode(sha2::Sha256::digest(b"{\"answer\":42}"))
    );
    let bad = format!("sha512-{}", base64::engine::general_purpose::STANDARD.encode(sha2::Sha512::digest(b"no")));
    env.eval(&format!(
        "globalThis.result='pending';const run=(i)=>fetch('/json',i).then(r=>r.text(),e=>e.name);\
         Promise.all([run({{integrity:'{good}'}}),run({{integrity:'{bad}'}}),run({{integrity:'{good} {bad}'}}),\
         run({{integrity:'md5-zzz'}}),run({{integrity:'{good}',mode:'no-cors'}})])\
         .then(v=>result=v.join('|'))"
    ));
    env.until("result!=='pending'");
    assert_eq!(env.eval("result"), "{\"answer\":42}|TypeError|TypeError|{\"answer\":42}|TypeError");
    env.finish_network_tasks();
    assert_eq!(env.page.fetch_usage(), (0, 0));
    let _ = server.release.send(());
}

/// A keepalive fetch outlives its document: disposal leaves the transport
/// running to completion, where an ordinary fetch is canceled.
#[test]
fn keepalive_fetch_survives_document_disposal() {
    let server = Server::new();
    let env = Env::new(&server.url);
    // The browser context keeps its client after a document goes.
    let context_client = env.page.net.borrow().clone().unwrap();
    let events = context_client.subscribe();
    env.eval("globalThis.result='pending';fetch('/gated',{keepalive:true}).then(()=>result='headers')");
    env.until("result!=='pending'");
    env.page.dispose();
    server.release.send(()).unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        match events.try_recv() {
            Ok(pyro_net::NetEvent::LoadingFinished { .. }) => break,
            Ok(pyro_net::NetEvent::LoadingFailed { error, .. }) => panic!("keepalive fetch failed: {error:?}"),
            Ok(_) => {}
            Err(_) => {
                assert!(std::time::Instant::now() < deadline, "no terminal event");
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }
}

/// `navigator.sendBeacon` sends a credentialed POST with its body and type
/// that no document state owns; over the keepalive quota it returns false.
#[test]
fn send_beacon_posts_its_body() {
    let server = Server::new();
    let env = Env::new(&server.url);
    assert_eq!(env.eval("navigator.sendBeacon('/beacon-text', 'hello')"), "true");
    let (first, headers, body) = recorded(&server, "/beacon-text");
    assert!(first.starts_with("POST "), "{first}");
    assert_eq!(body, b"hello");
    assert_eq!(header(&headers, "content-type"), Some("text/plain;charset=UTF-8"));
    assert_eq!(header(&headers, "sec-fetch-mode"), Some("no-cors"));
    assert_eq!(header(&headers, "sec-fetch-dest"), Some("empty"));
    assert_eq!(header(&headers, "referer"), Some(server.url.as_str()));
    assert_eq!(env.eval("navigator.sendBeacon('/beacon-empty')"), "true");
    let (first, _, body) = recorded(&server, "/beacon-empty");
    assert!(first.starts_with("POST ") && body.is_empty(), "{first}");
    assert_eq!(env.eval("navigator.sendBeacon('/beacon-params', new URLSearchParams({a:'1'}))"), "true");
    assert_eq!(recorded(&server, "/beacon-params").2, b"a=1");
    assert_eq!(env.eval("navigator.sendBeacon('/beacon-big', 'x'.repeat(70000))"), "false");
    env.page.dispose();
    let _ = server.release.send(());
}

/// XHR sends the document as its Referer, under the document's policy.
#[test]
fn xhr_sends_the_document_referrer() {
    let server = Server::new();
    let env = Env::new(&server.url);
    env.eval("globalThis.result='pending';const x=new XMLHttpRequest();x.open('GET','/json?xhr');x.onloadend=()=>result=x.status;x.send()");
    env.until("result!=='pending'");
    assert_eq!(header(&recorded(&server, "?xhr").1, "referer"), Some(server.url.as_str()));
    let _ = server.release.send(());
}
