//! HTMLMediaElement through the browser's event loop, as Chrome runs it with
//! a source it cannot play: `loadstart` then `error` (MEDIA_ERR_SRC_NOT_SUPPORTED)
//! for `src`, `error` at each `<source>`, and play() promises rejected.
use lp_browser::{BrowserThread, InitScript, NavigateOpts, PageId, StaticLoader};
use std::rc::Rc;
use std::time::{Duration, Instant};

fn eval(thread: &BrowserThread, page: PageId, source: &str) -> String {
    let realm = thread.browser.page(page).unwrap().main_frame.main_realm().unwrap();
    let scope = realm.scope().unwrap();
    scope.eval(source, None, 1).unwrap_or_else(|e| panic!("{}", e.details(&scope).text())).to_string().unwrap()
}
fn settle(thread: &mut BrowserThread) {
    assert_ne!(thread.run_until_idle(Some(Instant::now() + Duration::from_secs(3))), lp_runtime::RunOutcome::TimedOut);
}

const EVENTS: &str = "globalThis.events = []; for (const type of ['loadstart', 'error', 'emptied', 'abort', 'play', 'waiting', 'pause', 'timeupdate', 'volumechange', 'ratechange']) document.addEventListener(type, e => events.push((e.target.id || e.target.tagName) + ':' + type + ':' + (e.target.networkState ?? '')), true);";

fn setup(html: &str) -> (BrowserThread, PageId) {
    let mut thread = crate::browser_thread().unwrap();
    let page = thread.browser.create_page().unwrap();
    thread.browser.add_init_script(page, InitScript { id: "events".into(), world: None, source: EVENTS.into() });
    let loader = Rc::new(StaticLoader::default());
    loader.insert("http://site.test/", "text/html", html, Duration::ZERO);
    thread.browser.set_loader(Some(loader));
    thread.browser.navigate(page, NavigateOpts::address_bar("http://site.test/")).unwrap();
    settle(&mut thread);
    (thread, page)
}

#[test]
fn parsed_sources_fail_as_unsupported() {
    let (thread, page) = setup(
        "<audio id=a src=/a.mp3></audio><video id=v><source id=s1 src=a.webm><source id=s2 src=b.mp4></video><video id=e></video>",
    );
    // Each element's events in order (the elements' loads interleave with parsing).
    let of = |prefixes: &str| eval(&thread, page, &format!("events.filter(e => /^({prefixes}):/.test(e)).join(' ')"));
    assert_eq!(of("a"), "a:loadstart:2 a:error:3");
    assert_eq!(of("v|s1|s2"), "v:loadstart:2 s1:error: s2:error:");
    assert_eq!(of("e"), "");
    assert_eq!(
        eval(&thread, page, "var a = document.getElementById('a'); [a.networkState, a.error instanceof MediaError, a.error.code, a.error === a.error, a.currentSrc, a.readyState, a.paused].join()"),
        "3,true,4,true,http://site.test/a.mp3,0,true"
    );
    // `<source>` failures leave the element waiting for another source,
    // without a MediaError; no source at all leaves it empty.
    assert_eq!(
        eval(&thread, page, "var v = document.getElementById('v'), e = document.getElementById('e'); [v.networkState, v.error, v.currentSrc, e.networkState, e.NETWORK_EMPTY].join()"),
        "3,,http://site.test/b.mp4,0,0"
    );
}

#[test]
fn play_rejects_like_chrome_without_a_decodable_source() {
    let (mut thread, page) = setup("<p>media</p>");
    eval(
        &thread,
        page,
        "globalThis.results = []; var a = new Audio('/ding.mp3'); a.id = 'a'; document.body.append(a); a.play().then(() => results.push('resolved'), e => results.push('first:' + e.name)); a.currentTime = 0;",
    );
    settle(&mut thread);
    assert_eq!(eval(&thread, page, "events.join(' ')"), "a:play:2 a:waiting:2 a:loadstart:2 a:error:3");
    assert_eq!(eval(&thread, page, "results.join()"), "first:NotSupportedError");
    // Once the source failed, play() rejects at once.
    eval(&thread, page, "events.length = 0; a.play().catch(e => results.push('again:' + e.name + ':' + e.message));");
    settle(&mut thread);
    assert_eq!(eval(&thread, page, "results[1]"), "again:NotSupportedError:The element has no supported sources.");
    // A new src runs the load algorithm again: emptied, then a new failure.
    eval(&thread, page, "events.length = 0; a.src = '/other.ogg';");
    settle(&mut thread);
    assert_eq!(eval(&thread, page, "events.join(' ')"), "a:emptied:3 a:loadstart:2 a:error:3");
}

#[test]
fn pause_rejects_pending_play_and_fires_pause() {
    let (mut thread, page) = setup("<video id=v></video>");
    eval(
        &thread,
        page,
        "globalThis.results = []; var v = document.getElementById('v'); v.play().catch(e => results.push(e.name)); results.push(v.paused); v.pause(); results.push(v.paused);",
    );
    settle(&mut thread);
    assert_eq!(eval(&thread, page, "results.join()"), "false,true,AbortError");
    assert_eq!(eval(&thread, page, "events.join(' ')"), "v:play:0 v:waiting:0 v:timeupdate:0 v:pause:0");
}

#[test]
fn idl_state_is_settable_in_strict_code() {
    let (mut thread, page) = setup("<audio id=a muted></audio>");
    assert_eq!(
        eval(&thread, page, "(() => { 'use strict'; var a = document.getElementById('a'); a.currentTime = 12.5; a.volume = 0.25; a.playbackRate = 2; a.defaultPlaybackRate = 1.5; return [a.currentTime, a.volume, a.playbackRate, a.defaultPlaybackRate, a.muted, a.ended, a.seeking, a.buffered.length, a.seekable instanceof TimeRanges, a.played.length, a.textTracks.length, a.textTracks === a.textTracks].join(); })()"),
        "12.5,0.25,2,1.5,true,false,false,0,true,0,0,true"
    );
    assert_eq!(eval(&thread, page, "try { a.volume = 1.5 } catch (e) { e.name }"), "IndexSizeError");
    assert_eq!(eval(&thread, page, "try { a.volume = NaN } catch (e) { e.name }"), "TypeError");
    assert_eq!(eval(&thread, page, "try { a.playbackRate = 100 } catch (e) { e.name }"), "NotSupportedError");
    assert_eq!(eval(&thread, page, "try { a.buffered.start(0) } catch (e) { e.name }"), "IndexSizeError");
    // muted is not reflected; defaultMuted is.
    assert_eq!(
        eval(&thread, page, "a.muted = false; [a.muted, a.defaultMuted, a.hasAttribute('muted')].join()"),
        "false,true,true"
    );
    settle(&mut thread);
    assert_eq!(
        eval(&thread, page, "events.join(' ')"),
        "a:volumechange:0 a:ratechange:0 a:ratechange:0 a:volumechange:0"
    );
    assert_eq!(
        eval(&thread, page, "[HTMLMediaElement.HAVE_ENOUGH_DATA, a.NETWORK_NO_SOURCE, MediaError.MEDIA_ERR_SRC_NOT_SUPPORTED, a.canPlayType('audio/mpeg')].join()"),
        "4,3,4,"
    );
}

#[test]
fn script_inserted_media_and_late_sources_load() {
    let (mut thread, page) = setup("<div id=d></div>");
    // innerHTML-created elements load once inserted; a <source> added to an
    // element waiting for one is tried (and fails) too.
    eval(&thread, page, "document.getElementById('d').innerHTML = '<video id=v src=movie.webm></video><audio id=w><source id=s1 src=a.ogg></audio>';");
    settle(&mut thread);
    eval(&thread, page, "var s2 = document.createElement('source'); s2.id = 's2'; s2.src = 'b.ogg'; document.getElementById('w').append(s2);");
    settle(&mut thread);
    let of = |prefixes: &str| eval(&thread, page, &format!("events.filter(e => /^({prefixes}):/.test(e)).join(' ')"));
    assert_eq!(of("v"), "v:loadstart:2 v:error:3");
    assert_eq!(of("w|s1|s2"), "w:loadstart:2 s1:error: s2:error:");
    assert_eq!(eval(&thread, page, "document.getElementById('w').currentSrc"), "http://site.test/b.ogg");
    // srcObject runs the load algorithm and fails like a src.
    eval(&thread, page, "events.length = 0; var x = document.createElement('video'); x.id = 'x'; document.body.append(x); x.srcObject = new Blob([]);");
    settle(&mut thread);
    assert_eq!(
        eval(&thread, page, "events.join(' ') + '|' + x.error.code + '|' + (x.srcObject instanceof Blob)"),
        "x:loadstart:2 x:error:3|4|true"
    );
}
