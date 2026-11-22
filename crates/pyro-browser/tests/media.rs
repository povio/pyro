//! HTMLMediaElement through the browser's event loop, as Chrome runs it:
//! resources are fetched and their container headers read (small files
//! generated here), the ready states fire their events, playback advances a
//! clock on the page's time, and sources Chrome could not play fail with
//! MEDIA_ERR_SRC_NOT_SUPPORTED.
use pyro_browser::{BrowserThread, InitScript, NavigateOpts, PageId, StaticLoader};
use std::rc::Rc;
use std::time::{Duration, Instant};

fn eval(thread: &BrowserThread, page: PageId, source: &str) -> String {
    let realm = thread.browser.page(page).unwrap().main_frame.main_realm().unwrap();
    let scope = realm.scope().unwrap();
    scope.eval(source, None, 1).unwrap_or_else(|e| panic!("{}", e.details(&scope).text())).to_string().unwrap()
}
fn settle(thread: &mut BrowserThread) {
    assert_ne!(thread.run_until_idle(Some(Instant::now() + Duration::from_secs(3))), pyro_runtime::RunOutcome::TimedOut);
}
/// Moves the page clock (and its timers) `ms` ahead, then runs what came due.
fn advance(thread: &mut BrowserThread, ms: u64) {
    thread.browser.advance_time_explicit(Duration::from_millis(ms)).unwrap();
    settle(thread);
}
/// Whether a media element of the page has a playback timer.
fn has_timers(thread: &BrowserThread, page: PageId) -> bool {
    thread.browser.page(page).unwrap().main_frame.page_dom.as_ref().unwrap().media_timers() != 0
}

const EVENTS: &str = "globalThis.events = []; for (const type of ['loadstart', 'progress', 'suspend', 'durationchange', 'resize', 'loadedmetadata', 'loadeddata', 'canplay', 'canplaythrough', 'playing', 'error', 'emptied', 'abort', 'play', 'waiting', 'pause', 'timeupdate', 'seeking', 'seeked', 'ended', 'volumechange', 'ratechange']) document.addEventListener(type, e => events.push((e.target.id || e.target.tagName) + ':' + type + ':' + (e.target.networkState ?? '')), true); addEventListener('load', () => events.push('window:load')); globalThis.ev = () => events.filter(e => e !== 'window:load').join(' ');";

fn setup_with(html: &str, files: &[(&str, &str, Vec<u8>, Duration)]) -> (BrowserThread, PageId, Rc<StaticLoader>) {
    let mut thread = crate::browser_thread().unwrap();
    let page = thread.browser.create_page().unwrap();
    thread.browser.add_init_script(page, InitScript { id: "events".into(), world: None, source: EVENTS.into() });
    let loader = Rc::new(StaticLoader::default());
    loader.insert("http://site.test/", "text/html", html, Duration::ZERO);
    for (path, mime, body, delay) in files {
        loader.insert_bytes(&format!("http://site.test/{path}"), mime, body, *delay);
    }
    thread.browser.set_loader(Some(loader.clone()));
    thread.browser.navigate(page, NavigateOpts::address_bar("http://site.test/")).unwrap();
    settle(&mut thread);
    (thread, page, loader)
}

fn setup(html: &str) -> (BrowserThread, PageId) {
    let (thread, page, _) = setup_with(html, &[]);
    (thread, page)
}

/// Generated media files: headers that say what a real file would.
mod samples {
    fn mp4_box(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut out = ((payload.len() + 8) as u32).to_be_bytes().to_vec();
        out.extend_from_slice(kind);
        out.extend_from_slice(payload);
        out
    }

    /// An MP4 of `seconds` with one H.264 video track of `size`.
    pub fn mp4(seconds: f64, size: (u32, u32)) -> Vec<u8> {
        let mut mvhd = vec![0; 12];
        mvhd.extend_from_slice(&1000u32.to_be_bytes());
        mvhd.extend_from_slice(&((seconds * 1000.0) as u32).to_be_bytes());
        mvhd.extend_from_slice(&[0; 80]);
        let mut tkhd = vec![0; 4 + 20 + 16];
        for v in [0x10000u32, 0, 0, 0, 0x10000, 0, 0, 0, 0x4000_0000, size.0 << 16, size.1 << 16] {
            tkhd.extend_from_slice(&v.to_be_bytes());
        }
        let mut hdlr = vec![0; 8];
        hdlr.extend_from_slice(b"vide");
        hdlr.extend_from_slice(&[0; 13]);
        let mut stsd = vec![0, 0, 0, 0, 0, 0, 0, 1];
        stsd.extend(mp4_box(b"avc1", &[0; 70]));
        let stbl = mp4_box(b"stbl", &mp4_box(b"stsd", &stsd));
        let mdia = mp4_box(b"mdia", &[mp4_box(b"hdlr", &hdlr), mp4_box(b"minf", &stbl)].concat());
        let trak = mp4_box(b"trak", &[mp4_box(b"tkhd", &tkhd), mdia].concat());
        let moov = mp4_box(b"moov", &[mp4_box(b"mvhd", &mvhd), trak].concat());
        [mp4_box(b"ftyp", b"isom\0\0\x02\0isomavc1"), moov, mp4_box(b"mdat", &[0; 16])].concat()
    }

    fn ebml(id: u32, payload: &[u8]) -> Vec<u8> {
        let id = id.to_be_bytes();
        let mut out = id[id.iter().position(|&b| b != 0).unwrap()..].to_vec();
        out.push(0x01);
        out.extend_from_slice(&(payload.len() as u64).to_be_bytes()[1..]);
        out.extend_from_slice(payload);
        out
    }

    /// A WebM of `seconds` with an Opus audio track only.
    pub fn webm_audio(seconds: f64) -> Vec<u8> {
        let header = ebml(0x1A45DFA3, &ebml(0x4282, b"webm"));
        let info = ebml(0x1549A966, &ebml(0x4489, &(seconds * 1000.0).to_be_bytes()));
        let tracks = ebml(0x1654AE6B, &ebml(0xAE, &[ebml(0x83, &[2]), ebml(0x86, b"A_OPUS")].concat()));
        [header, ebml(0x18538067, &[info, tracks].concat())].concat()
    }

    /// A 16-bit mono PCM WAV of `seconds` at 8 kHz.
    pub fn wav(seconds: f64) -> Vec<u8> {
        let data = vec![0u8; (seconds * 16000.0) as usize];
        let mut out = b"RIFF".to_vec();
        out.extend_from_slice(&((36 + data.len()) as u32).to_le_bytes());
        out.extend_from_slice(b"WAVEfmt \x10\0\0\0\x01\0\x01\0");
        out.extend_from_slice(&8000u32.to_le_bytes());
        out.extend_from_slice(&16000u32.to_le_bytes());
        out.extend_from_slice(b"\x02\0\x10\0data");
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend(data);
        out
    }
}

fn mp4_file(path: &'static str, seconds: f64, delay: u64) -> (&'static str, &'static str, Vec<u8>, Duration) {
    (path, "video/mp4", samples::mp4(seconds, (320, 240)), Duration::from_millis(delay))
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
        "globalThis.results = []; var a = new Audio('/ding.mp3'); a.defaultMuted = true; a.id = 'a'; document.body.append(a); a.play().then(() => results.push('resolved'), e => results.push('first:' + e.name)); a.currentTime = 0;",
    );
    settle(&mut thread);
    assert_eq!(eval(&thread, page, "ev()"), "a:play:2 a:waiting:2 a:loadstart:2 a:error:3");
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
    let (mut thread, page) = setup("<video id=v muted></video>");
    eval(
        &thread,
        page,
        "globalThis.results = []; var v = document.getElementById('v'); v.play().catch(e => results.push(e.name)); results.push(v.paused); v.pause(); results.push(v.paused);",
    );
    settle(&mut thread);
    assert_eq!(eval(&thread, page, "results.join()"), "false,true,AbortError");
    assert_eq!(eval(&thread, page, "ev()"), "v:play:0 v:waiting:0 v:timeupdate:0 v:pause:0");
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
    assert_eq!(eval(&thread, page, "ev()"), "a:volumechange:0 a:ratechange:0 a:ratechange:0 a:volumechange:0");
    assert_eq!(
        eval(&thread, page, "[HTMLMediaElement.HAVE_ENOUGH_DATA, a.NETWORK_NO_SOURCE, MediaError.MEDIA_ERR_SRC_NOT_SUPPORTED, a.canPlayType('audio/mpeg'), a.canPlayType('video/webm; codecs=\"av01.0.04M.08, opus\"'), a.canPlayType('video/mp4'), a.canPlayType('video/x-fictional')].join()"),
        "4,3,4,probably,probably,maybe,"
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

#[test]
fn a_playable_video_walks_the_ready_states_before_the_load_event() {
    let (thread, page, loader) = setup_with("<video id=v src=v.mp4></video>", &[mp4_file("v.mp4", 2.5, 40)]);
    assert_eq!(
        eval(&thread, page, "events.join(' ')"),
        "v:loadstart:2 v:progress:2 v:durationchange:2 v:resize:2 v:loadedmetadata:2 v:loadeddata:2 window:load v:canplay:2 v:canplaythrough:2 v:suspend:1"
    );
    assert_eq!(
        eval(&thread, page, "var v = document.getElementById('v'); [v.readyState, v.networkState, v.duration, v.videoWidth, v.videoHeight, v.paused, v.currentTime, v.buffered.end(0), v.seekable.length, v.error].join()"),
        "4,1,2.5,320,240,true,0,2.5,1,"
    );
    // A Media request went through the page's resource loader.
    assert!(loader.log.borrow().iter().any(|u| u == "http://site.test/v.mp4"));
    // Nothing plays: no timer is left.
    assert!(!has_timers(&thread, page));
}

#[test]
fn playback_advances_on_the_page_clock_and_ends() {
    let (mut thread, page, _) = setup_with("<video id=v muted src=v.mp4></video>", &[mp4_file("v.mp4", 1.0, 0)]);
    eval(&thread, page, "events.length = 0; globalThis.results = []; var v = document.getElementById('v'); v.play().then(() => results.push('resolved:' + v.paused));");
    settle(&mut thread);
    assert_eq!(eval(&thread, page, "events.join(' ') + '|' + results.join()"), "v:play:1 v:playing:1|resolved:false");
    assert!(has_timers(&thread, page), "a playing element keeps one timer");
    advance(&mut thread, 300);
    let t: f64 = eval(&thread, page, "v.currentTime").parse().unwrap();
    assert!((0.3..0.35).contains(&t), "currentTime {t}");
    assert!(eval(&thread, page, "events.includes('v:timeupdate:1')") == "true");
    // Pausing stops the clock and its timer.
    eval(&thread, page, "events.length = 0; v.pause();");
    settle(&mut thread);
    assert!(!has_timers(&thread, page), "a paused element has no timer");
    let paused_at = eval(&thread, page, "v.currentTime");
    advance(&mut thread, 500);
    assert_eq!(eval(&thread, page, "v.currentTime"), paused_at);
    // Double speed to the end: timeupdate, pause, ended.
    eval(&thread, page, "events.length = 0; v.playbackRate = 2; v.play();");
    settle(&mut thread);
    advance(&mut thread, 250);
    advance(&mut thread, 250);
    assert_eq!(
        eval(&thread, page, "[v.ended, v.paused, v.currentTime, v.played.length, v.played.end(0)].join()"),
        "true,true,1,1,1"
    );
    assert_eq!(
        eval(&thread, page, "events.filter(e => !/timeupdate/.test(e)).join(' ')"),
        "v:ratechange:1 v:play:1 v:playing:1 v:pause:1 v:ended:1"
    );
    assert!(!has_timers(&thread, page));
    // play() at the end starts over.
    eval(&thread, page, "events.length = 0; v.play();");
    settle(&mut thread);
    assert_eq!(eval(&thread, page, "[v.currentTime < 0.1, v.ended, v.paused].join()"), "true,false,false");
    assert!(eval(&thread, page, "events.join(' ')").contains("v:seeking:1"));
}

#[test]
fn seeking_and_loop() {
    let (mut thread, page, _) = setup_with("<video id=v muted loop src=v.mp4></video>", &[mp4_file("v.mp4", 1.0, 0)]);
    eval(&thread, page, "events.length = 0; var v = document.getElementById('v'); v.currentTime = 0.75; globalThis.during = [v.seeking, v.currentTime].join();");
    settle(&mut thread);
    assert_eq!(
        eval(&thread, page, "during + '|' + v.seeking + '|' + events.join(' ')"),
        "true,0.75|false|v:seeking:1 v:timeupdate:1 v:seeked:1"
    );
    // Out of range seeks clamp to the duration.
    eval(&thread, page, "v.currentTime = 9;");
    settle(&mut thread);
    assert_eq!(eval(&thread, page, "v.currentTime"), "1");
    // Looping playback seeks back to the start instead of ending.
    eval(&thread, page, "v.currentTime = 0.9; events.length = 0; v.play();");
    settle(&mut thread);
    advance(&mut thread, 150);
    advance(&mut thread, 0);
    let t: f64 = eval(&thread, page, "v.currentTime").parse().unwrap();
    assert!(t < 0.2, "looped back: {t}");
    assert_eq!(
        eval(&thread, page, "[v.paused, v.ended, events.includes('v:ended:1'), events.includes('v:seeked:1')].join()"),
        "false,false,false,true"
    );
    // Removing a playing element pauses it.
    eval(&thread, page, "events.length = 0; v.onpause = v.ontimeupdate = e => events.push('v:' + e.type + ':' + v.networkState); v.remove();");
    settle(&mut thread);
    assert_eq!(eval(&thread, page, "[v.paused, events.join(' ')].join('|')"), "true|v:timeupdate:1 v:pause:1");
    assert!(!has_timers(&thread, page));
}

#[test]
fn autoplay_follows_chromes_policy() {
    let (thread, page, _) = setup_with(
        "<video id=loud autoplay src=a.mp4></video><video id=quiet autoplay muted src=b.mp4></video>",
        &[mp4_file("a.mp4", 1.0, 0), mp4_file("b.mp4", 1.0, 0)],
    );
    assert_eq!(
        eval(
            &thread,
            page,
            "[loud.paused, quiet.paused, events.includes('quiet:playing:2'), events.includes('loud:play:1')].join()"
        ),
        "true,false,true,false"
    );
    // Unmuted play() needs user activation.
    eval(&thread, page, "globalThis.results = []; loud.play().catch(e => results.push(e.name));");
    let mut thread = thread;
    settle(&mut thread);
    assert_eq!(eval(&thread, page, "results.join()"), "NotAllowedError");
    thread.browser.page(page).unwrap().main_frame.page_dom.as_ref().unwrap().notify_user_activation();
    eval(&thread, page, "loud.play().then(() => results.push('ok'));");
    settle(&mut thread);
    assert_eq!(eval(&thread, page, "[results.join(), loud.paused].join('|')"), "NotAllowedError,ok|false");
}

#[test]
fn source_children_skip_unplayable_types_and_garbage() {
    let (thread, page, loader) = setup_with(
        "<video id=v><source id=s1 src=a.ogv type='video/ogg; codecs=theora'><source id=s2 src=bad.mp4 type=video/mp4><source id=s3 src=c.mp4></video><video id=g src=bad.mp4></video>",
        &[
            ("bad.mp4", "video/mp4", b"<!doctype html>not a movie".to_vec(), Duration::ZERO),
            mp4_file("c.mp4", 3.0, 0),
        ],
    );
    let of = |prefixes: &str| {
        eval(
            &thread,
            page,
            &format!("events.filter(e => /^({prefixes}):/.test(e)).map(e => e.replace(/:\\d*$/, '')).join(' ')"),
        )
    };
    assert_eq!(
        of("v|s1|s2|s3").split(' ').take(4).collect::<Vec<_>>().join(" "),
        "v:loadstart s1:error s2:error v:progress"
    );
    assert_eq!(eval(&thread, page, "[v.currentSrc, v.duration, v.readyState].join()"), "http://site.test/c.mp4,3,4");
    // The unplayable type was never fetched.
    assert!(!loader.log.borrow().iter().any(|u| u.ends_with("a.ogv")));
    assert_eq!(of("g"), "g:loadstart g:error");
    assert_eq!(
        eval(&thread, page, "[g.error.code, g.error.message].join()"),
        "4,DEMUXER_ERROR_COULD_NOT_OPEN: FFmpegDemuxer: open context failed"
    );
}

#[test]
fn preload_none_waits_for_play_and_audio_containers_load() {
    let (mut thread, page, loader) = setup_with(
        "<audio id=a preload=none muted src=a.wav></audio><audio id=w src=b.webm></audio>",
        &[
            ("a.wav", "audio/wav", samples::wav(0.5), Duration::ZERO),
            ("b.webm", "audio/webm", samples::webm_audio(1.25), Duration::ZERO),
        ],
    );
    let of = |prefixes: &str| eval(&thread, page, &format!("events.filter(e => /^({prefixes}):/.test(e)).join(' ')"));
    assert_eq!(of("a"), "a:loadstart:2 a:suspend:1");
    assert_eq!(eval(&thread, page, "[a.readyState, a.duration].join()"), "0,NaN");
    assert!(!loader.log.borrow().iter().any(|u| u.ends_with("a.wav")));
    assert_eq!(eval(&thread, page, "[w.duration, w.readyState, w.videoWidth].join()"), "1.25,4,");
    eval(&thread, page, "a.play();");
    settle(&mut thread);
    assert_eq!(eval(&thread, page, "[a.duration, a.readyState, a.paused].join()"), "0.5,4,false");
    let events = eval(&thread, page, "events.filter(e => /^a:/.test(e)).join(' ')");
    assert!(events.ends_with("a:canplay:2 a:playing:2 a:canplaythrough:2 a:suspend:1"), "{events}");
}

#[test]
fn long_resources_are_read_in_part() {
    // Past the 16 MB a media element keeps: an MP4 whose moov follows its
    // media data is read with a Range request from the moov box, and a WAV
    // keeps its declared length (both used to be buffered whole, and failed
    // past the network's 256 MB body limit).
    let mp4 = samples::mp4(2.5, (320, 240));
    let moov_at = mp4.windows(4).position(|w| w == b"moov").unwrap() - 4;
    let moov_len = u32::from_be_bytes(mp4[moov_at..moov_at + 4].try_into().unwrap()) as usize;
    let mdat_len = 20 << 20;
    let mut mdat = (mdat_len as u32).to_be_bytes().to_vec();
    mdat.extend_from_slice(b"mdat");
    mdat.resize(mdat_len, 0);
    let late = [&mp4[..moov_at], &mdat, &mp4[moov_at..moov_at + moov_len]].concat();
    let offset = moov_at + mdat_len;
    let (thread, page, loader) = setup_with(
        "<video id=v src=late.mp4></video><audio id=w src=long.wav></audio>",
        &[
            ("late.mp4", "video/mp4", late, Duration::ZERO),
            ("long.wav", "audio/wav", samples::wav(1500.0), Duration::ZERO),
        ],
    );
    assert_eq!(
        eval(&thread, page, "var v = document.getElementById('v'), w = document.getElementById('w'); [v.duration, v.videoWidth, v.readyState, w.duration, w.readyState].join()"),
        "2.5,320,4,1500,4"
    );
    let log = loader.log.borrow();
    assert!(log.iter().any(|u| *u == format!("http://site.test/late.mp4 bytes={offset}-")), "{log:?}");
}

#[test]
fn removing_the_current_source_keeps_the_pointer_in_place() {
    // HTML: the source pointer keeps its place when the `<source>` being
    // tried is removed, so a failure moves on to the next candidate rather
    // than starting over (which fired `error` at failed candidates again).
    let (thread, page, _) = setup_with(
        "<video id=v1><source id=a1 src=bad.mp4 onerror='setTimeout(() => b1.remove(), 10)'><source id=b1 src=slow.mp4> <source id=c1 src=c.mp4></video>\
         <video id=v2><source id=a2 src=bad.mp4> <source id=b2 src=bad.mp4 onerror='this.remove()'><source id=c2 src=c.mp4></video>",
        &[
            ("bad.mp4", "video/mp4", b"<!doctype html>not a movie".to_vec(), Duration::ZERO),
            ("slow.mp4", "video/mp4", b"<!doctype html>not a movie".to_vec(), Duration::from_millis(100)),
            mp4_file("c.mp4", 3.0, 0),
        ],
    );
    let errors = |n: &str| {
        eval(
            &thread,
            page,
            &format!("events.filter(e => /^[abc]{n}:error/.test(e)).map(e => e.split(':')[0]).join(' ')"),
        )
    };
    // b1 left the document before its error (the document listener misses it).
    assert_eq!(errors("1"), "a1");
    assert_eq!(errors("2"), "a2 b2");
    assert_eq!(
        eval(&thread, page, "[v1.currentSrc, v1.readyState, v2.currentSrc, v2.readyState].join()"),
        "http://site.test/c.mp4,4,http://site.test/c.mp4,4"
    );
}

#[test]
fn track_fetches_use_the_media_parent_and_keep_completed_cues_on_reattachment() {
    let (mut thread, page, loader) = setup_with(
        r#"<!doctype html><video id="media"><track id="captions" default src="lyrics.vtt"></video>"#,
        &[
            ("lyrics.vtt", "text/vtt", b"WEBVTT\n\none\n00:00.000 --> 00:02.000\nhello\n".to_vec(), Duration::ZERO),
            ("next.vtt", "text/vtt", b"WEBVTT\n\ntwo\n00:02.000 --> 00:04.000\nagain\n".to_vec(), Duration::ZERO),
        ],
    );
    assert_eq!(eval(&thread, page,
        "globalThis.video = document.querySelector('video'); globalThis.caption = document.querySelector('track'); \
         globalThis.firstCue = caption.track.cues[0]; [caption.readyState, firstCue.id, firstCue.text].join('|')"),
        "2|one|hello");
    eval(&thread, page, "caption.src = './lyrics.vtt'");
    settle(&mut thread);
    assert_eq!(eval(&thread, page, "[caption.readyState, caption.track.cues[0] === firstCue].join('|')"), "2|true");
    eval(&thread, page, "caption.remove(); caption.src = 'lyrics.vtt'; video.appendChild(caption)");
    settle(&mut thread);
    assert_eq!(eval(&thread, page, "[caption.readyState, caption.track.cues[0] === firstCue, video.textTracks.length].join('|')"), "2|true|1");
    assert_eq!(loader.log.borrow().iter().filter(|url| url.ends_with("/lyrics.vtt")).count(), 1);
    eval(&thread, page, "caption.remove(); caption.src = 'next.vtt'; video.appendChild(caption)");
    settle(&mut thread);
    assert_eq!(eval(&thread, page, "[caption.readyState, caption.track.cues.length, caption.track.cues[0].id].join('|')"), "2|1|two");
    assert_eq!(loader.log.borrow().iter().filter(|url| url.ends_with("/next.vtt")).count(), 1);
    // Without a CORS setting the track destination uses same-origin mode.
    eval(&thread, page, "globalThis.errorSeen = false; caption.onerror = () => { errorSeen = true }; caption.src='https://other.test/captions.vtt'");
    settle(&mut thread);
    assert_eq!(eval(&thread, page, "[caption.readyState, caption.track.cues.length, errorSeen].join('|')"), "3|0|true");
    assert!(!loader.log.borrow().iter().any(|url| url.starts_with("https://other.test/")));
}
