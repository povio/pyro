fn evaluate(source: &str) -> String {
    let mut browser = crate::browser_thread().unwrap();
    let page = browser.browser.create_page().unwrap();
    let realm = browser.browser.page(page).unwrap().main_frame.main_realm().unwrap().clone();
    let scope = realm.scope().unwrap();
    scope.eval(source, None, 1).unwrap_or_else(|e| panic!("{}", e.details(&scope).text()));
    scope.eval("JSON.stringify({reads,ran,constructed})", None, 1).unwrap().to_string().unwrap()
}

#[test]
fn queue_microtask_does_not_read_author_promise_species() {
    assert_eq!(
        evaluate(
            r#"
        globalThis.reads=0;globalThis.ran=0;globalThis.constructed=0;
        Object.defineProperty(Promise,Symbol.species,{get(){reads++;return Promise}});
        queueMicrotask(()=>ran++);
    "#
        ),
        r#"{"reads":0,"ran":1,"constructed":0}"#
    );
}

#[test]
fn queue_microtask_does_not_recursively_enter_a_scheduling_promise_constructor() {
    assert_eq!(
        evaluate(
            r#"
        globalThis.reads=0;globalThis.ran=0;globalThis.constructed=0;
        class SchedulingPromise extends Promise {
            constructor(executor) {
                super(executor);
                if (++constructed < 16) queueMicrotask(()=>{});
            }
        }
        Object.defineProperty(Promise,Symbol.species,{value:SchedulingPromise});
        queueMicrotask(()=>ran++);
    "#
        ),
        r#"{"reads":0,"ran":1,"constructed":0}"#
    );
}

#[test]
fn queue_microtask_ignores_mutated_promise_prototype_constructor() {
    assert_eq!(
        evaluate(
            r#"
        globalThis.reads=0;globalThis.ran=0;globalThis.constructed=0;
        Object.defineProperty(Promise.prototype,'constructor',{get(){reads++;throw Error('author constructor')}});
        queueMicrotask(()=>ran++);
    "#
        ),
        r#"{"reads":0,"ran":1,"constructed":0}"#
    );
}

#[test]
fn mutation_observer_native_microtasks_ignore_author_constructor_hooks() {
    assert_eq!(
        evaluate(
            r#"
        globalThis.reads=0;globalThis.ran=0;globalThis.constructed=0;
        const observer=new MutationObserver(()=>ran++);
        observer.observe(document.body,{childList:true});
        Object.defineProperty(Promise.prototype,'constructor',{get(){reads++;throw Error('author constructor')}});
        document.body.appendChild(document.createElement('p'));
    "#
        ),
        r#"{"reads":0,"ran":1,"constructed":0}"#
    );
}
