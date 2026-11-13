//! WICG Observable (https://wicg.github.io/observable/) on the real
//! JavaScriptCore: ref-counted producers, Subscriber brand and argument
//! checks, teardown and abort ordering through DOM abort algorithms,
//! "report an exception", the iterable conversions of `Observable.from()`
//! and the operators' Web IDL conversions.

use crate::support::network::Env;

fn env() -> Env {
    let env = Env::new("http://observable.test/");
    env.realm.set_exception_reporter(Some(pyro_webapi::lifecycle::report_exception));
    env
}

#[test]
fn subscriber_methods_check_their_receiver_and_arguments() {
    let env = env();
    assert_eq!(
        env.eval(
            r#"(() => {
              let subscriber;
              new Observable(s => { subscriber = s; }).subscribe();
              const kind = f => { try { f(); return "no"; } catch (e) { return e.constructor.name; } };
              const { next, error, complete, addTeardown } = subscriber;
              return [
                kind(() => subscriber.next()), kind(() => subscriber.error()), kind(() => subscriber.addTeardown()),
                kind(() => subscriber.addTeardown(1)), kind(() => next(1)), kind(() => error(1)), kind(() => complete()),
                kind(() => addTeardown(() => {})), kind(() => new Subscriber()), kind(() => new Observable()),
                kind(() => new Observable(1)), kind(() => subscriber.next(undefined)), subscriber.active,
              ].join();
            })()"#
        ),
        "TypeError,TypeError,TypeError,TypeError,TypeError,TypeError,TypeError,TypeError,TypeError,TypeError,TypeError,no,true"
    );
    // Web IDL conversions of subscribe()'s arguments; handlers run with an
    // undefined this value.
    assert_eq!(
        env.eval(
            r#"(() => {
              const o = new Observable(s => { s.next(1); s.complete(); });
              const kind = f => { try { f(); return "no"; } catch (e) { return e.constructor.name; } };
              const seen = [];
              const observer = { next() { "use strict"; seen.push(this === undefined); }, complete() { "use strict"; seen.push(this === undefined); } };
              o.subscribe(observer);
              return [
                kind(() => o.subscribe(1)), kind(() => o.subscribe({ next: 1 })), kind(() => o.subscribe({}, 1)),
                kind(() => o.subscribe({}, { signal: {} })), kind(() => o.subscribe(null, null)),
                kind(() => o.subscribe({ next: undefined }, { signal: undefined })), seen.join("/"),
              ].join();
            })()"#
        ),
        "TypeError,TypeError,TypeError,TypeError,no,no,true/true"
    );
}

#[test]
fn subscriptions_share_one_producer_until_the_last_one_ends() {
    let env = env();
    assert_eq!(
        env.eval(
            r#"(() => {
              const log = [];
              let active;
              const source = new Observable(s => { active = s; log.push("producer"); s.addTeardown(() => log.push("teardown")); });
              const a = new AbortController(), b = new AbortController(), c = new AbortController();
              source.subscribe(v => log.push("a" + v), { signal: a.signal });
              source.subscribe(v => log.push("b" + v), { signal: b.signal });
              source.subscribe(v => log.push("c" + v), { signal: c.signal });
              active.next(1);
              b.abort(); log.push("b gone");
              active.next(2);
              a.abort(); log.push("a gone");
              c.abort(); log.push("c gone");
              // The producer ended: the next subscription starts a new one.
              source.subscribe({ complete: () => log.push("done") });
              active.complete();
              source.subscribe();
              return log.join();
            })()"#
        ),
        "producer,a1,b1,c1,b gone,a2,c2,a gone,teardown,c gone,producer,teardown,done,producer"
    );
    // The observers are snapshotted for each value; one that subscribes
    // with an aborted signal never joins.
    assert_eq!(
        env.eval(
            r#"(() => {
              const log = [];
              const source = new Observable(s => { s.next(1); s.next(2); s.complete(); });
              source.subscribe(v => {
                log.push(v + "-first");
                if (v === 1) {
                  source.subscribe(v => log.push(v + "-second"));
                  source.subscribe(v => log.push(v + "-aborted"), { signal: AbortSignal.abort() });
                }
              });
              return log.join();
            })()"#
        ),
        "1-first,2-first,2-second"
    );
}

#[test]
fn one_signal_serves_ended_and_live_subscriptions() {
    let env = env();
    // The unsubscription steps of ended subscriptions (and the rejection
    // steps of settled promises) leave the signal; the live one still ends.
    env.eval(
        r#"globalThis.log = [];
           (() => {
             const ac = new AbortController();
             const done = new Observable(s => { s.addTeardown(() => log.push("done teardown")); s.next(1); s.complete(); });
             for (let i = 0; i < 3; i++) done.subscribe({ complete: () => log.push("complete") }, { signal: ac.signal });
             done.toArray({ signal: ac.signal }).then(v => log.push("array " + v), e => log.push("rejected " + e));
             new Observable(s => { s.addTeardown(() => log.push("live teardown")); }).subscribe({}, { signal: ac.signal });
             Promise.resolve().then(() => { ac.abort("end"); log.push("aborted"); });
           })()"#,
    );
    assert_eq!(
        env.eval("log.join()"),
        "done teardown,complete,done teardown,complete,done teardown,complete,done teardown,array 1,live teardown,aborted"
    );
}

#[test]
fn an_aborted_signal_closes_the_subscriber_before_the_producer_runs() {
    let env = env();
    assert_eq!(
        env.eval(
            r#"(() => {
              const log = [];
              const source = new Observable(s => {
                log.push(s.active, s.signal.aborted, s.signal.reason);
                s.addTeardown(() => log.push("teardown 1"));
                log.push("after 1");
                s.addTeardown(() => log.push("teardown 2"));
                s.next(1); s.complete();
              });
              source.subscribe({ next: v => log.push(v), complete: () => log.push("complete") }, { signal: AbortSignal.abort("gone") });
              return log.join();
            })()"#
        ),
        "false,true,gone,teardown 1,after 1,teardown 2"
    );
}

#[test]
fn unsubscription_runs_through_abort_algorithms_before_the_abort_event() {
    let env = env();
    // Consumer-initiated: each upstream subscriber closes (abort event, then
    // teardowns) inside its downstream signal's abort algorithms.
    assert_eq!(
        env.eval(
            r#"(() => {
              const log = [];
              const make = (name, source) => new Observable(s => {
                s.signal.addEventListener("abort", () => log.push(name + " abort"));
                s.addTeardown(() => log.push(name + " teardown " + s.signal.reason));
                if (source) source.subscribe({}, { signal: s.signal });
              });
              const ac = new AbortController();
              ac.signal.addEventListener("abort", () => log.push("outer abort"));
              make("down", make("middle", make("up"))).subscribe({}, { signal: ac.signal });
              ac.abort("x");
              return log.join();
            })()"#
        ),
        "up abort,up teardown x,middle abort,middle teardown x,down abort,down teardown x,outer abort"
    );
    // Producer-initiated, and through the operators' internal subscriptions.
    assert_eq!(
        env.eval(
            r#"(() => {
              const log = [];
              let inner;
              const source = new Observable(s => {
                s.addTeardown(() => log.push("source teardown"));
                s.signal.addEventListener("abort", () => log.push("source abort"));
              });
              const down = new Observable(s => {
                inner = s;
                s.addTeardown(() => log.push("down teardown"));
                s.signal.addEventListener("abort", () => log.push("down abort"));
                source.map(v => v).subscribe(() => {}, { signal: s.signal });
              });
              down.subscribe();
              inner.complete();
              return log.join();
            })()"#
        ),
        "source abort,source teardown,down abort,down teardown"
    );
    // catch(): the handler's Observable is unsubscribed inside the outer
    // signal's abort algorithms. switchMap(): the inner subscription follows
    // a dependent signal, which aborts after the source's subscription.
    assert_eq!(
        env.eval(
            r#"(() => {
              const log = [];
              const failing = new Observable(s => s.error(new Error("source")));
              const caught = failing.catch(() => new Observable(s => {
                s.addTeardown(() => log.push("inner teardown"));
                s.signal.addEventListener("abort", () => log.push("inner abort"));
              }));
              const ac = new AbortController();
              ac.signal.addEventListener("abort", () => log.push("outer abort"));
              caught.subscribe({}, { signal: ac.signal });
              ac.abort();
              const source = new Observable(s => {
                s.next(1);
                s.addTeardown(() => log.push("source teardown"));
                s.signal.onabort = () => log.push("source onabort");
              });
              const inner = new Observable(s => {
                s.addTeardown(() => log.push("switch teardown"));
                s.signal.onabort = () => log.push("switch onabort");
              });
              const ac2 = new AbortController();
              source.switchMap(() => inner).subscribe({}, { signal: ac2.signal });
              ac2.abort();
              return log.join();
            })()"#
        ),
        "inner abort,inner teardown,outer abort,source onabort,source teardown,switch onabort,switch teardown"
    );
}

#[test]
fn dependent_signals_abort_after_their_source() {
    let env = env();
    // DOM "signal abort": the dependents take the reason first, then the
    // source runs its abort steps, then each dependent runs its own.
    assert_eq!(
        env.eval(
            r#"(() => {
              const log = [];
              const ac = new AbortController();
              const any = AbortSignal.any([ac.signal]);
              const nested = AbortSignal.any([any]);
              ac.signal.addEventListener("abort", () => log.push("source:" + any.aborted + ":" + (any.reason === ac.signal.reason)));
              any.addEventListener("abort", () => log.push("dependent"));
              nested.addEventListener("abort", () => log.push("nested"));
              ac.abort("why");
              log.push(nested.reason, AbortSignal.any([ac.signal, new AbortController().signal]).reason);
              return log.join();
            })()"#
        ),
        "source:true:true,dependent,nested,why,why"
    );
    // forEach() rejects from an abort algorithm of its dependent signal: after
    // the abort event of the caller's signal, before the subscriber's.
    env.eval(
        r#"globalThis.order = [];
           (() => {
             const source = new Observable(s => {
               s.signal.addEventListener("abort", () => Promise.resolve().then(() => order.push("inner")));
             });
             const ac = new AbortController();
             ac.signal.addEventListener("abort", () => Promise.resolve().then(() => order.push("outer")));
             source.forEach(() => {}, { signal: ac.signal }).catch(e => order.push("rejected " + e));
             ac.abort("stop");
           })()"#,
    );
    assert_eq!(env.eval("order.join()"), "outer,rejected stop,inner");
}

#[test]
fn unhandled_errors_are_reported_to_the_global() {
    let env = env();
    assert_eq!(
        env.eval(
            r#"(() => {
              const seen = [];
              addEventListener("error", e => { seen.push([e.message.includes("custom"), e.lineno > 0, e.colno > 0, e.error.message].join("/")); e.preventDefault(); });
              const error = new Error("custom 1");
              // No error handler; the initializer throws; error() after the end.
              new Observable(s => s.error(error)).subscribe({ next() {} });
              new Observable(() => { throw new Error("custom 2"); }).subscribe();
              new Observable(s => { s.complete(); s.error(new Error("custom 3")); }).subscribe({ error: () => seen.push("handler") });
              // A handler that throws, and a teardown that throws.
              new Observable(s => { s.addTeardown(() => { throw new Error("custom 5"); }); s.next(1); s.complete(); })
                .subscribe(() => { throw new Error("custom 4"); });
              return seen.join();
            })()"#
        ),
        "true/true/true/custom 1,true/true/true/custom 2,true/true/true/custom 3,true/true/true/custom 4,true/true/true/custom 5"
    );
    // A value that is no Error is reported at the script that pushed it.
    assert_eq!(
        env.eval(
            r#"(() => {
              let seen;
              addEventListener("error", e => { seen = [e.message, e.filename, e.lineno, e.colno > 0, e.error].join("/"); e.preventDefault(); }, { once: true });
              new Observable(s => {
                s.error("text");
              }).subscribe();
              return seen;
            })()"#
        ),
        "Uncaught text/fetch-test.js/5/true/text"
    );
    // toArray(): the first error() rejects, a later one is reported.
    env.eval(
        r#"globalThis.out = [];
           addEventListener("error", e => out.push("reported " + e.error.message + " " + (e.lineno > 0)));
           new Observable(s => { s.error(new Error("first")); s.error(new Error("second")); }).toArray().catch(e => out.push("rejected " + e.message));"#,
    );
    assert_eq!(env.eval("out.join()"), "reported second true,rejected first");
}

#[test]
fn from_iterable_shares_the_iteration_and_closes_the_iterator_on_abort() {
    let env = env();
    assert_eq!(
        env.eval(
            r#"(() => {
              const log = [];
              const source = Observable.from([1, 2, 3, 4]);
              source.subscribe({
                next: v => {
                  log.push(v + "a");
                  if (v === 2) source.subscribe({ next: v => log.push(v + "b"), complete: () => log.push("b done") });
                },
                complete: () => log.push("a done"),
              });
              return log.join();
            })()"#
        ),
        "1a,2a,3a,3b,4a,4b,a done,b done"
    );
    assert_eq!(
        env.eval(
            r#"(() => {
              const log = [];
              const iterable = (limit, ret) => ({
                [Symbol.iterator]() {
                  return { val: 0, next() { return { value: this.val, done: this.val++ === limit }; }, return: ret };
                },
              });
              // Aborted midway: return() is called once, with no arguments.
              let ac = new AbortController();
              Observable.from(iterable(10, function () { log.push("return " + arguments.length + " " + this.val); return {}; }))
                .subscribe(v => { if (v === 2) ac.abort(); }, { signal: ac.signal });
              // Run to completion: the iterator is exhausted, not closed.
              ac = new AbortController();
              Observable.from(iterable(2, () => log.push("unexpected"))).subscribe({ complete: () => log.push("complete") }, { signal: ac.signal });
              ac.abort();
              // return() must give an Object, and its exception leaves abort().
              for (const ret of [() => {}, () => { throw new Error("from return"); }]) {
                ac = new AbortController();
                Observable.from(iterable(10, ret)).subscribe(v => {
                  if (v !== 1) return;
                  try { ac.abort(); log.push("no throw"); } catch (e) { log.push(e.constructor.name + ": " + e.message); }
                }, { signal: ac.signal });
                log.push(ac.signal.aborted);
              }
              // The getter aborts the subscription: the iterator is made but
              // neither stepped nor closed.
              ac = new AbortController();
              let calls = 0;
              const aborting = {};
              Object.defineProperty(aborting, Symbol.iterator, { get() {
                if (calls++ > 0) ac.abort();
                return () => ({ next() { log.push("next"); return { done: true }; }, return() { log.push("return"); return {}; } });
              } });
              Observable.from(aborting).subscribe({ complete: () => log.push("complete") }, { signal: ac.signal });
              log.push(calls);
              return log.join("|");
            })()"#
        ),
        "return 0 3|complete|TypeError: The return() method of the iterator did not return an Object|true|Error: from return|true|2"
    );
}

#[test]
fn from_async_iterable_reacts_in_microtasks_and_closes_with_the_reason() {
    let env = env();
    env.eval(
        r#"globalThis.log = [];
           (() => {
             const iterable = {
               [Symbol.asyncIterator]() {
                 return {
                   val: 0,
                   next() { log.push("next " + this.val); return { value: this.val, done: this.val++ === 10 }; },
                   return(reason) { log.push("return " + reason); return { done: true }; },
                 };
               },
             };
             const ac = new AbortController();
             Observable.from(iterable).subscribe(v => { log.push("got " + v); if (v === 1) ac.abort("enough"); }, { signal: ac.signal });
             log.push("subscribed");
           })()"#,
    );
    assert_eq!(env.eval("log.join()"), "next 0,subscribed,got 0,next 1,got 1,return enough");
    // An exception of next() is a rejection: the observer hears of it in a
    // microtask. One of [Symbol.asyncIterator]() is delivered at once.
    env.eval(
        r#"globalThis.log = [];
           Observable.from({ [Symbol.asyncIterator]() { return { next() { throw new Error("next"); } }; } })
             .subscribe({ error: e => log.push(e.message) });
           log.push("sync");
           Observable.from({ [Symbol.asyncIterator]() { throw new Error("method"); } }).subscribe({ error: e => log.push(e.message) });
           log.push("end");"#,
    );
    assert_eq!(env.eval("log.join()"), "sync,method,end,next");
    // No async iterator: the sync one is wrapped. A return() that gives no
    // Object, or throws, is an unhandled rejection, not an exception.
    env.eval(
        r#"globalThis.log = [];
           addEventListener("unhandledrejection", e => { log.push("unhandled " + e.reason.constructor.name + " " + e.reason.message); e.preventDefault(); });
           (() => {
             const fallback = { get [Symbol.asyncIterator]() { return this.method; }, method() { return null; }, [Symbol.iterator]() { return [7, 8][Symbol.iterator](); } };
             const source = Observable.from(fallback);
             fallback.method = undefined;
             source.subscribe({ next: v => log.push(v), complete: () => log.push("complete") });
             for (const ret of [() => {}, () => { throw new RangeError("from return"); }]) {
               const ac = new AbortController();
               Observable.from({ [Symbol.asyncIterator]() { return { next() { return { value: 1, done: false }; }, return: ret }; } })
                 .subscribe(() => { ac.abort(); log.push("aborted"); }, { signal: ac.signal });
             }
           })()"#,
    );
    env.until("log.length >= 7");
    assert_eq!(
        env.eval("log.join('|')"),
        "aborted|aborted|7|8|complete|unhandled TypeError The return() method of the async iterator did not return an Object|unhandled RangeError from return"
    );
}

#[test]
fn operators_convert_their_arguments_as_web_idl_does() {
    let env = env();
    assert_eq!(
        env.eval(
            r#"(() => {
              const out = [];
              const source = new Observable(s => { s.next(1); s.next(2); s.next(3); s.complete(); });
              const run = o => { const got = []; o.subscribe({ next: v => got.push(v), complete: () => got.push("c") }); return got.join(""); };
              // unsigned long long: -1 wraps to the maximum, NaN and Infinity are 0.
              out.push(run(source.take(-1)), run(source.drop(-1)), run(source.take(NaN)), run(source.take(Infinity)), run(source.take(2.9)), run(source.drop("1")));
              const kind = f => { try { f(); return "no"; } catch (e) { return e.constructor.name; } };
              out.push(kind(() => source.take()), kind(() => source.map()), kind(() => source.map(1)), kind(() => source.takeUntil(1)),
                kind(() => source.inspect(1)), kind(() => source.inspect({ next: 1 })), kind(() => source.inspect()), kind(() => Observable.from("abc")),
                kind(() => Observable.prototype.map.call({}, x => x)));
              // Callbacks run with an undefined this value.
              const thises = [];
              source.map(function (v) { "use strict"; thises.push(this); return v; }).filter(function () { "use strict"; thises.push(this); return true; })
                .inspect({ next() { "use strict"; thises.push(this); } }).subscribe();
              out.push(thises.every(t => t === undefined) && thises.length === 9);
              return out.join();
            })()"#
        ),
        "123c,c,c,c,12c,23c,TypeError,TypeError,TypeError,TypeError,TypeError,TypeError,no,TypeError,TypeError,true"
    );
    // Promise-returning operators reject instead of throwing.
    env.eval(
        r#"globalThis.out = [];
           (async () => {
             const source = new Observable(s => { s.next(1); s.next(2); s.complete(); });
             for (const make of [() => source.forEach(), () => source.every(1), () => source.toArray({ signal: 1 }), () => Observable.prototype.first.call({}),
                                 () => source.last({ signal: AbortSignal.abort(new TypeError("aborted")) })]) {
               const p = make();
               out.push(p instanceof Promise);
               await p.then(() => out.push("resolved"), e => out.push(e.constructor.name));
             }
             out.push((await source.toArray()).join("+"), await source.first(), await source.last(), await source.reduce((a, v) => a + v, 10));
             out.push("done");
           })()"#,
    );
    env.until("out.includes('done')");
    assert_eq!(
        env.eval("out.join()"),
        "true,TypeError,true,TypeError,true,TypeError,true,TypeError,true,TypeError,1+2,1,2,13,done"
    );
}

#[test]
fn inspect_abort_handler_is_for_consumer_unsubscription_only() {
    let env = env();
    assert_eq!(
        env.eval(
            r#"(() => {
              const log = [];
              const source = new Observable(s => { s.addTeardown(() => log.push("teardown")); s.next(1); s.complete(); });
              const ac = new AbortController();
              source.inspect({ abort: r => log.push("abort " + r), complete: () => log.push("inspect complete") })
                .subscribe({ complete: () => log.push("complete") }, { signal: ac.signal });
              ac.abort("late");
              const endless = new Observable(s => { s.addTeardown(() => log.push("endless teardown")); });
              const ac2 = new AbortController();
              endless.inspect({ abort: r => log.push("abort " + r) }).subscribe({}, { signal: ac2.signal });
              ac2.abort("now");
              return log.join();
            })()"#
        ),
        "teardown,inspect complete,complete,abort now,endless teardown"
    );
}

#[test]
fn when_observes_events_until_the_subscription_ends() {
    let env = env();
    assert_eq!(
        env.eval(
            r#"(() => {
              const log = [];
              const target = new EventTarget();
              const ac = new AbortController();
              const clicks = target.when("click");
              clicks.subscribe(e => log.push("a " + e.type), { signal: ac.signal });
              clicks.take(1).subscribe({ next: e => log.push("b " + e.type), complete: () => log.push("b done") });
              target.dispatchEvent(new Event("click"));
              ac.abort();
              target.dispatchEvent(new Event("click"));
              const kind = f => { try { f(); return "no"; } catch (e) { return e.constructor.name; } };
              log.push(kind(() => target.when()), kind(() => target.when("x", 1)), kind(() => target.when(Symbol())));
              return log.join();
            })()"#
        ),
        "a click,b click,b done,TypeError,TypeError,TypeError"
    );
}
