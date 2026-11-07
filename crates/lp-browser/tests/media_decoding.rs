//! Decoded media through the browser's event loop: Web Audio
//! (`OfflineAudioContext` rendering, `decodeAudioData`, a real-time
//! `AudioContext` on the page clock) and `<video>` frames (`drawImage`,
//! `createImageBitmap`, `requestVideoFrameCallback`, render snapshots).
//! Media files are lp-media's fixtures: a 440 Hz sine, and ten 64x48
//! frames at 10 fps where frame N is rgb(N*25, 255-N*25, 128).
use lp_browser::{BrowserThread, NavigateOpts, PageId, StaticLoader};
use std::rc::Rc;
use std::time::{Duration, Instant};

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(format!("{}/../lp-media/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

fn eval(thread: &BrowserThread, page: PageId, source: &str) -> String {
    let realm = thread.browser.page(page).unwrap().main_frame.main_realm().unwrap();
    let scope = realm.scope().unwrap();
    scope.eval(source, None, 1).unwrap_or_else(|e| panic!("{}", e.details(&scope).text())).to_string().unwrap()
}

fn settle(thread: &mut BrowserThread) {
    assert_ne!(thread.run_until_idle(Some(Instant::now() + Duration::from_secs(3))), lp_runtime::RunOutcome::TimedOut);
}

fn wait_for(thread: &mut BrowserThread, page: PageId, condition: &str) {
    let outcome = thread.run_until(Some(Instant::now() + Duration::from_secs(10)), |b| {
        let scope = b.page(page).unwrap().main_frame.main_realm().unwrap().scope().unwrap();
        scope.eval(condition, None, 1).is_ok_and(|v| v.to_bool())
    });
    if outcome != lp_runtime::RunOutcome::Done {
        let scope = thread.browser.page(page).unwrap().main_frame.main_realm().unwrap().scope().unwrap();
        let state =
            scope.eval("JSON.stringify(globalThis.results ?? null)", None, 1).ok().and_then(|v| v.to_string().ok());
        panic!("timed out waiting for {condition}: {state:?}");
    }
}

fn setup(html: &str, files: &[&str]) -> (BrowserThread, PageId) {
    let mut thread = crate::browser_thread().unwrap();
    let page = thread.browser.create_page().unwrap();
    let loader = Rc::new(StaticLoader::default());
    loader.insert("http://site.test/", "text/html", html, Duration::ZERO);
    for name in files {
        let mime = match name.rsplit('.').next() {
            Some("mp4") => "video/mp4",
            Some("webm") => "video/webm",
            Some("wav") => "audio/wav",
            Some("mp3") => "audio/mpeg",
            _ => "application/octet-stream",
        };
        loader.insert_bytes(&format!("http://site.test/{name}"), mime, &fixture(name), Duration::ZERO);
    }
    thread.browser.set_loader(Some(loader));
    thread.browser.navigate(page, NavigateOpts::address_bar("http://site.test/")).unwrap();
    settle(&mut thread);
    (thread, page)
}

#[test]
fn offline_context_renders_sources_gain_and_automation() {
    let (mut thread, page) = setup("<!doctype html>", &[]);
    eval(
        &thread,
        page,
        "const ctx = new OfflineAudioContext(2, 512, 8000);
         globalThis.states = [ctx.state];
         ctx.onstatechange = () => states.push(ctx.state);
         // A constant 1 from frame 128 through a gain ramping 0 -> 1 over 256 frames.
         const src = ctx.createConstantSource();
         const gain = new GainNode(ctx, { gain: 0 });
         gain.gain.setValueAtTime(0, 0).linearRampToValueAtTime(1, 256 / 8000);
         src.connect(gain).connect(ctx.destination);
         src.start(128 / 8000);
         src.stop(384 / 8000);
         // A buffer source looping [0.1, 0.2, 0.3, 0.4] at half speed, into channel sums.
         const buffer = ctx.createBuffer(1, 4, 8000);
         buffer.copyToChannel(new Float32Array([0.1, 0.2, 0.3, 0.4]), 0);
         const looped = new AudioBufferSourceNode(ctx, { buffer, loop: true, playbackRate: 0.5 });
         looped.connect(ctx.destination);
         looped.start(0);
         looped.stop(8 / 8000);
         globalThis.ended = [];
         src.onended = () => ended.push('const');
         looped.addEventListener('ended', () => ended.push('loop'));
         ctx.startRendering().then(b => { globalThis.rendered = b; });
         ctx.oncomplete = e => { globalThis.completed = e.renderedBuffer; };",
    );
    wait_for(&mut thread, page, "globalThis.rendered !== undefined");
    let out = eval(
        &thread,
        page,
        "const l = rendered.getChannelData(0), r = rendered.getChannelData(1);
         const f = (x) => Math.round(x * 1000) / 1000;
         JSON.stringify({
           size: [rendered.numberOfChannels, rendered.length, rendered.sampleRate],
           loop: Array.from(l.slice(0, 10), f),
           ramp: [l[127], l[128], l[200], l[383], l[384], l[511]].map(f),
           same: l.every((v, i) => v === r[i]),
           states, ended: ended.sort(), complete: completed === rendered,
         })",
    );
    assert_eq!(
        out,
        r#"{"size":[2,512,8000],"loop":[0.1,0.15,0.2,0.25,0.3,0.35,0.4,0.25,0,0],"ramp":[0,0.5,0.781,1,0,0],"same":true,"states":["suspended","running","closed"],"ended":["const","loop"],"complete":true}"#
    );
}

#[test]
fn oscillator_and_param_errors() {
    let (mut thread, page) = setup("<!doctype html>", &[]);
    eval(
        &thread,
        page,
        "const ctx = new OfflineAudioContext({ length: 4800, sampleRate: 48000 });
         new OscillatorNode(ctx, { frequency: 1000 }).connect(ctx.destination).context.destination;
         const playing = new OscillatorNode(ctx, { frequency: 1000 });
         playing.connect(ctx.destination);
         playing.start();
         ctx.startRendering().then(b => { globalThis.rendered = b; });
         // Errors on another oscillator (automation of this one would be heard).
         const osc = ctx.createOscillator();
         osc.start();
         const errors = [];
         const t = (f) => { try { f(); errors.push('ok'); } catch (e) { errors.push(e.name); } };
         t(() => osc.start());
         t(() => osc.frequency.exponentialRampToValueAtTime(0, 1));
         t(() => osc.frequency.setValueAtTime(1, -1));
         t(() => osc.frequency.setValueCurveAtTime([1], 0, 1));
         t(() => osc.frequency.setValueCurveAtTime([1, 2], 0, 1));
         t(() => osc.frequency.setValueAtTime(1, 0.5));
         t(() => new OfflineAudioContext(0, 1, 44100));
         t(() => new AudioBuffer({ length: 1, sampleRate: 1 }));
         t(() => ctx.createGain().connect(new OfflineAudioContext(1, 1, 8000).destination));
         t(() => new AudioNode());
         globalThis.errors = errors;",
    );
    wait_for(&mut thread, page, "globalThis.rendered !== undefined");
    assert_eq!(
        eval(&thread, page, "errors.join()"),
        "InvalidStateError,RangeError,RangeError,InvalidStateError,ok,NotSupportedError,NotSupportedError,NotSupportedError,InvalidAccessError,TypeError"
    );
    // 1 kHz for 0.1 s: 100 periods, peaks at 1.
    assert_eq!(
        eval(
            &thread,
            page,
            "const d = rendered.getChannelData(0); let up = 0; for (let i = 1; i < d.length; i++) if (d[i - 1] <= 0 && d[i] > 0) up++;
             [up, Math.round(Math.max(...d) * 100) / 100, d[0], Math.round(d[12] * 1000) / 1000].join()"
        ),
        "100,1,0,1"
    );
}

/// A JS expression making a fresh ArrayBuffer of fixture `name`.
fn array_buffer(name: &str) -> String {
    let bytes: Vec<String> = fixture(name).iter().map(u8::to_string).collect();
    format!("new Uint8Array([{}]).buffer", bytes.join(","))
}

#[test]
fn decode_audio_data_decodes_resamples_and_detaches() {
    let (mut thread, page) = setup("<!doctype html>", &[]);
    eval(
        &thread,
        page,
        &format!(
            "globalThis.wavBytes = () => {}; globalThis.mp3Bytes = () => {};",
            array_buffer("sine.wav"),
            array_buffer("sine.mp3")
        ),
    );
    eval(
        &thread,
        page,
        "const ctx = new OfflineAudioContext(1, 1, 48000);
         globalThis.results = {};
         (async () => {
           const wav = wavBytes();
           const viaCallback = new Promise(r => ctx.decodeAudioData(wav, b => r(b)));
           results.detached = wav.byteLength;
           const b = await viaCallback;
           const d = b.getChannelData(0);
           let up = 0; for (let i = 1; i < d.length; i++) if (d[i - 1] <= 0 && d[i] > 0) up++;
           results.wav = [b.sampleRate, b.length, b.numberOfChannels, up];
           const mp3 = await ctx.decodeAudioData(mp3Bytes());
           results.mp3 = [mp3.sampleRate, mp3.length > 23900 && mp3.length < 25600];
           try { await ctx.decodeAudioData(new TextEncoder().encode('not audio').buffer); } catch (e) { results.garbage = e.name; }
           try { await ctx.decodeAudioData(wav); } catch (e) { results.again = e.name; }
           try { await ctx.decodeAudioData('x'); } catch (e) { results.type = e.name; }
           results.done = true;
         })();",
    );
    wait_for(&mut thread, page, "results.done === true");
    assert_eq!(
        eval(&thread, page, "JSON.stringify(results)"),
        r#"{"detached":0,"wav":[48000,24000,1,219],"mp3":[48000,true],"garbage":"EncodingError","again":"DataCloneError","type":"TypeError","done":true}"#
    );
}

#[test]
fn realtime_context_needs_activation_and_runs_on_the_page_clock() {
    let (mut thread, page) = setup("<!doctype html>", &[]);
    eval(
        &thread,
        page,
        "globalThis.quiet = new AudioContext(); globalThis.resumed = false; quiet.resume().then(() => resumed = true);",
    );
    settle(&mut thread);
    assert_eq!(
        eval(&thread, page, "[quiet.state, resumed, quiet.currentTime, quiet.sampleRate].join()"),
        "suspended,false,0,48000"
    );
    thread.browser.page(page).unwrap().main_frame.page_dom.as_ref().unwrap().notify_user_activation();
    eval(
        &thread,
        page,
        "globalThis.ctx = new AudioContext({ sampleRate: 44100 }); globalThis.ended = 0;
         const b = ctx.createBuffer(1, 44100 * 5, 44100);
         globalThis.s = ctx.createBufferSource(); s.buffer = b; s.onended = () => ended++; s.connect(ctx.destination); s.start();",
    );
    settle(&mut thread);
    assert_eq!(eval(&thread, page, "[ctx.state, ended].join()"), "running,0");
    eval(&thread, page, "globalThis.t0 = ctx.currentTime");
    thread.browser.advance_time_explicit(Duration::from_millis(250)).unwrap();
    settle(&mut thread);
    assert_eq!(
        eval(&thread, page, "[ctx.currentTime - t0 > 0.2 && ctx.currentTime - t0 < 0.3, ended].join()"),
        "true,0"
    );
    eval(&thread, page, "s.stop(ctx.currentTime)");
    settle(&mut thread);
    assert_eq!(eval(&thread, page, "ended"), "1");
    eval(&thread, page, "ctx.suspend().then(() => globalThis.at = ctx.currentTime)");
    settle(&mut thread);
    thread.browser.advance_time_explicit(Duration::from_millis(500)).unwrap();
    settle(&mut thread);
    assert_eq!(eval(&thread, page, "[ctx.state, ctx.currentTime === at].join()"), "suspended,true");
}

/// The frame index of an RGB pixel.
const FRAME_OF: &str = "globalThis.frameOf = (p) => Math.round((p[0] / 25 + (255 - p[1]) / 25) / 2);";

fn check_video_frames(file: &str) {
    let (mut thread, page) = setup(
        &format!("<!doctype html><video id=v src=/{file}></video><canvas id=c width=64 height=48></canvas>"),
        &[file],
    );
    eval(&thread, page, FRAME_OF);
    eval(
        &thread,
        page,
        "const v = document.getElementById('v'), c = document.getElementById('c').getContext('2d');
         globalThis.draw = () => { c.drawImage(v, 0, 0); return frameOf(c.getImageData(32, 24, 1, 1).data); };",
    );
    assert_eq!(eval(&thread, page, "[v.readyState, v.videoWidth, v.videoHeight, draw()].join()"), "4,64,48,0");
    eval(&thread, page, "v.currentTime = 0.55");
    wait_for(&mut thread, page, "!v.seeking");
    assert_eq!(eval(&thread, page, "draw()"), "5");
    eval(&thread, page, "v.currentTime = 0.31");
    wait_for(&mut thread, page, "!v.seeking");
    assert_eq!(eval(&thread, page, "draw()"), "3");
    eval(&thread, page, "createImageBitmap(v).then(b => globalThis.bitmap = [b.width, b.height])");
    wait_for(&mut thread, page, "globalThis.bitmap !== undefined");
    assert_eq!(eval(&thread, page, "bitmap.join()"), "64,48");
    // The render snapshot paints the frame shown (no poster).
    let dom = thread.browser.page(page).unwrap().main_frame.page_dom.clone().unwrap();
    let frames = dom.video_bitmaps();
    assert_eq!(frames.len(), 1);
    let (_, w, h, rgba) = &frames[0];
    assert_eq!((*w, *h), (64, 48));
    let at = (24 * 64 + 32) * 4;
    let pixel = format!("[{},{},{}]", rgba[at], rgba[at + 1], rgba[at + 2]);
    assert_eq!(eval(&thread, page, &format!("frameOf({pixel})")), "3");
}

#[test]
fn av1_mp4_frames_draw_and_paint() {
    check_video_frames("frames-av1.mp4");
}

#[test]
fn av1_webm_frames_draw_and_paint() {
    check_video_frames("frames-av1.webm");
}

#[test]
fn h264_frames_draw_and_paint() {
    check_video_frames("frames-h264.mp4");
}

#[test]
fn video_frame_callbacks_run_per_presented_frame() {
    let (mut thread, page) =
        setup("<!doctype html><video id=v muted poster=/none.png src=/frames-av1.mp4></video>", &["frames-av1.mp4"]);
    eval(
        &thread,
        page,
        "const v = document.getElementById('v'); globalThis.seen = [];
         const each = (now, meta) => { seen.push([meta.mediaTime, meta.presentedFrames, meta.width, meta.height, typeof now]); if (seen.length < 4) v.requestVideoFrameCallback(each); };
         v.requestVideoFrameCallback(each);
         const cancelled = v.requestVideoFrameCallback(() => seen.push('cancelled'));
         v.cancelVideoFrameCallback(cancelled);",
    );
    wait_for(&mut thread, page, "seen.length === 1");
    // The first frame, presented while paused; nothing new until playback.
    assert_eq!(eval(&thread, page, "JSON.stringify(seen)"), r#"[[0,1,64,48,"number"]]"#);
    thread.browser.advance_time_explicit(Duration::from_millis(300)).unwrap();
    settle(&mut thread);
    assert_eq!(eval(&thread, page, "seen.length"), "1");
    eval(&thread, page, "v.play()");
    for _ in 0..4 {
        thread.browser.advance_time_explicit(Duration::from_millis(100)).unwrap();
        settle(&mut thread);
    }
    // Each call presents a later frame, counting up.
    assert_eq!(
        eval(
            &thread,
            page,
            "seen.length === 4 && seen.every((s, i) => i === 0 || (s[0] > seen[i - 1][0] && s[1] === seen[i - 1][1] + 1)) ? 'ok' : JSON.stringify(seen)"
        ),
        "ok"
    );
    assert_eq!(eval(&thread, page, "v.getVideoPlaybackQuality().totalVideoFrames"), "4");
}

#[test]
fn video_with_a_poster_paints_it_until_played() {
    let (mut thread, page) =
        setup("<!doctype html><video id=v muted poster=/p.png src=/frames-h264.mp4></video>", &["frames-h264.mp4"]);
    let dom = thread.browser.page(page).unwrap().main_frame.page_dom.clone().unwrap();
    assert!(dom.video_bitmaps().is_empty());
    eval(&thread, page, "document.getElementById('v').play()");
    settle(&mut thread);
    assert_eq!(dom.video_bitmaps().len(), 1);
}

#[test]
fn can_play_type_follows_the_decoders() {
    let (thread, page) = setup("<!doctype html>", &[]);
    assert_eq!(
        eval(
            &thread,
            page,
            "const v = document.createElement('video');
             ['video/mp4; codecs=\"avc1.42E01E\"', 'video/mp4; codecs=\"av01.0.04M.08\"', 'video/webm; codecs=\"av01.0.04M.08, opus\"',
              'video/webm; codecs=\"vp9, opus\"', 'video/webm; codecs=vp8', 'video/mp4; codecs=\"hvc1.1.6.L93.B0\"', 'audio/ogg; codecs=opus'].map(t => v.canPlayType(t)).join()"
        ),
        "probably,probably,probably,,,,probably"
    );
}
