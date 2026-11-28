//! Navigation Timing (the `navigation` entry, `performance.timing`,
//! `performance.navigation`), User Timing's timing attribute names and
//! PerformanceObserver (performance-timeline/, user-timing/, navigation-timing/).
use pyro_browser::{BrowserThread, NavigateOpts, PageId, StaticLoader};
use std::rc::Rc;
use std::time::{Duration, Instant};

fn eval(t: &BrowserThread, page: PageId, source: &str) -> String {
    let s = t.browser.page(page).unwrap().main_frame.main_realm().unwrap().scope().unwrap();
    s.eval(source, None, 1).unwrap_or_else(|e| panic!("{}", e.details(&s).text())).to_string().unwrap()
}

fn settle(t: &mut BrowserThread) {
    assert_eq!(t.run_until_idle(Some(Instant::now() + Duration::from_secs(2))), pyro_runtime::RunOutcome::Idle);
}

fn load(html: &str) -> (BrowserThread, PageId) {
    let mut t = crate::browser_thread().unwrap();
    let loader = Rc::new(StaticLoader::default());
    loader.insert("http://site.test/page", "text/html", html, Duration::ZERO);
    t.browser.set_loader(Some(loader));
    let page = t.browser.create_page().unwrap();
    t.browser.navigate(page, NavigateOpts::address_bar("http://site.test/page")).unwrap();
    settle(&mut t);
    (t, page)
}

#[test]
fn the_navigation_entry_records_the_document_lifecycle() {
    let (mut t, page) =
        load("<body><p>x</p><script>for (let i = 0; i < 20000; i++) document.body.append('')</script></body>");
    assert_eq!(
        eval(
            &t,
            page,
            "const [n] = performance.getEntriesByType('navigation'); \
             [performance.getEntries().filter(e => e.entryType == 'navigation').length, n.entryType, n.name, n.type, n.initiatorType, \
              n instanceof PerformanceNavigationTiming, n instanceof PerformanceResourceTiming, n.startTime, n.fetchStart, \
              n.domInteractive <= n.domContentLoadedEventStart, n.domContentLoadedEventStart <= n.domContentLoadedEventEnd, \
              n.domContentLoadedEventEnd <= n.domComplete, n.domComplete <= n.loadEventStart, n.loadEventStart <= n.loadEventEnd, \
              n.loadEventEnd > 0, n.duration == n.loadEventEnd, n.toJSON().type, typeof n.toJSON().domInteractive, \
              performance.getEntriesByName('http://site.test/page').length].join()"
        ),
        "1,navigation,http://site.test/page,navigate,navigation,true,true,0,0,true,true,true,true,true,true,true,navigate,number,1"
    );
    eval(&t, page, "location.reload()");
    settle(&mut t);
    assert_eq!(
        eval(&t, page, "performance.getEntriesByType('navigation')[0].type + ' ' + performance.navigation.type"),
        "reload 1"
    );
}

#[test]
fn legacy_timing_is_in_epoch_milliseconds() {
    let (t, page) = load("<p>x</p>");
    assert_eq!(
        eval(
            &t,
            page,
            "const p = performance.timing; \
             [p === performance.timing, p.navigationStart === Math.round(performance.timeOrigin), \
              p.fetchStart >= p.navigationStart, p.loadEventEnd >= p.domContentLoadedEventStart, \
              p.domContentLoadedEventStart >= p.navigationStart, p.unloadEventStart, p.redirectStart, \
              performance.navigation.type, performance.navigation.redirectCount, \
              Object.keys(p.toJSON()).length, typeof performance.toJSON().timing].join()"
        ),
        "true,true,true,true,true,0,0,0,0,21,object"
    );
    // User Timing resolves the attribute names; a step that never ran is
    // an InvalidAccessError.
    assert_eq!(
        eval(
            &t,
            page,
            "[performance.measure('load', 'navigationStart', 'loadEventEnd').duration >= 0, \
              performance.measure('m', 'domInteractive').startTime == performance.getEntriesByType('navigation')[0].domInteractive, \
              (() => { try { performance.measure('x', 'unloadEventStart'); } catch (e) { return e.name } })()].join()"
        ),
        "true,true,InvalidAccessError"
    );
}

#[test]
fn observers_get_marks_and_measures_in_a_task() {
    let (mut t, page) = load("<p>x</p>");
    assert_eq!(
        eval(
            &t,
            page,
            "window.seen = []; \
             window.o = new PerformanceObserver(function (list, obs, options) { \
               seen.push([this === o, obs === o, list instanceof PerformanceObserverEntryList, \
                 list.getEntries().map(e => e.name).join('+'), list.getEntriesByType('measure').length, \
                 options.droppedEntriesCount].join(':')); }); \
             o.observe({type: 'mark'}); o.observe({type: 'measure'}); \
             performance.mark('b', {startTime: 5}); performance.mark('a', {startTime: 1}); performance.measure('m'); \
             seen.length"
        ),
        "0"
    );
    settle(&mut t);
    assert_eq!(eval(&t, page, "seen.join()"), "true:true:true:m+a+b:1:0");
    // takeRecords empties the buffer; disconnect stops the callbacks.
    assert_eq!(
        eval(&t, page, "performance.mark('c'); const r = o.takeRecords(); o.disconnect(); performance.mark('d'); r.map(e => e.name).join() + o.takeRecords().length"),
        "c0"
    );
    settle(&mut t);
    assert_eq!(eval(&t, page, "seen.length"), "1");
}

#[test]
fn observe_validates_its_options() {
    let (mut t, page) = load("<p>x</p>");
    assert_eq!(
        eval(
            &t,
            page,
            "const f = () => {}; const err = (g) => { try { g(); return 'ok' } catch (e) { return e.name } }; \
             [PerformanceObserver.supportedEntryTypes.join('|'), \
              PerformanceObserver.supportedEntryTypes === PerformanceObserver.supportedEntryTypes, \
              err(() => new PerformanceObserver(f).observe({})), \
              err(() => new PerformanceObserver(f).observe({type: 'mark', entryTypes: ['mark']})), \
              err(() => new PerformanceObserver(f).observe({entryTypes: 'mark'})), \
              err(() => { const o = new PerformanceObserver(f); o.observe({entryTypes: ['mark']}); o.observe({type: 'measure'}) }), \
              err(() => { const o = new PerformanceObserver(f); o.observe({type: 'mark'}); o.observe({entryTypes: ['measure']}) }), \
              err(() => new PerformanceObserver(f).observe({type: 'resource'})), \
              err(() => new PerformanceObserver()), err(() => PerformanceObserver(f))].join()"
        ),
        "element|event|first-input|largest-contentful-paint|layout-shift|long-animation-frame|longtask|mark|measure|navigation|paint|resource,true,TypeError,TypeError,TypeError,InvalidModificationError,InvalidModificationError,ok,TypeError,TypeError"
    );
    // buffered: entries from before observe() are delivered too.
    eval(
        &t,
        page,
        "performance.mark('early'); window.got = []; \
         new PerformanceObserver(l => got.push(...l.getEntries().map(e => e.name))).observe({type: 'mark', buffered: true}); \
         performance.mark('late')",
    );
    settle(&mut t);
    assert_eq!(eval(&t, page, "got.join()"), "early,late");
}

#[test]
fn observers_see_the_navigation_and_paint_entries_when_they_arrive() {
    let (t, page) = load(
        "<head><script>window.got = []; \
         new PerformanceObserver(l => l.getEntries().forEach(e => got.push(e.entryType + ':' + e.name + ':' + (e.entryType != 'navigation' || e.loadEventEnd > 0)))) \
           .observe({entryTypes: ['navigation', 'paint']});</script></head><body><p>x</p></body>",
    );
    assert_eq!(
        eval(&t, page, "got.sort().join()"),
        "navigation:http://site.test/page:true,paint:first-contentful-paint:true,paint:first-paint:true"
    );
}
