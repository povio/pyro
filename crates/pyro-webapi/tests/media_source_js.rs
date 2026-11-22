//! The minimal `MediaSource` surface: constructible, closed, feature
//! detection false, and `URL.createObjectURL(mediaSource)` yields a blob URL
//! that fetch and XHR cannot resolve.

use std::cell::RefCell;
use std::rc::Rc;

use pyro_dom::parse::{parse_document, DomRef};
use pyro_dom::Dom;
use pyro_js::{ClassDef, Engine, JsClass, Realm, RealmOptions};
use pyro_webapi::*;

struct TestWindowData {
    page: Rc<PageDom>,
    target: EventTargetData,
}

impl WindowHost for TestWindowData {
    fn page(&self) -> &Rc<PageDom> {
        &self.page
    }
    fn event_target(&self) -> &EventTargetData {
        &self.target
    }
}

struct TestWindow;
impl JsClass for TestWindow {
    const NAME: &'static str = "Window";
    type Data = TestWindowData;
    type Parent = EventTarget;
    fn define(def: &mut ClassDef<Self>) {
        def.upcast(|w: &TestWindowData| &w.target);
        define_members::<TestWindow>(def);
        def.getter("window", |s, _w| Ok(s.global().value()));
    }
}

struct Env {
    #[allow(dead_code)]
    engine: Engine,
    page: Rc<PageDom>,
    main: Realm,
}

/// Loads no sub-resources.
struct NoFetch;
impl pyro_render::Fetcher for NoFetch {
    fn fetch(&self, url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(Err(format!("{url}: offline")));
    }
}

fn env(html: &str) -> Env {
    pyro_layout::install(std::sync::Arc::new(NoFetch));
    let engine = Engine::new().expect("engine");
    register(&engine);
    let window_class = engine.register::<TestWindow>();
    let dom: DomRef = Rc::new(RefCell::new(Dom::new()));
    dom.borrow_mut().set_resource_tracking(true);
    let doc = parse_document(&dom, "http://localhost:5173/app", html);
    dom.borrow_mut().set_document_active(doc, true).unwrap();
    let page = PageDom::new(dom, doc);
    let mut vp = page.viewport();
    vp.width = 1440;
    vp.height = 900;
    page.set_viewport(vp);
    page.layout.borrow_mut().style_mut().discover_style_elements(&page.dom.borrow());
    let realm = engine
        .create_realm(RealmOptions { name: String::new(), global_class: Some(window_class), origin: None })
        .expect("realm");
    let key = page.window_target().unwrap_or_else(|| page.create_window_target(page.document()));
    realm
        .set_global_data::<TestWindow>(TestWindowData {
            page: page.clone(),
            target: EventTargetData::window(&page, key),
        })
        .unwrap();
    install(&realm.scope().unwrap(), page.clone()).unwrap();
    Env { engine, page, main: realm }
}

fn eval(realm: &Realm, src: &str) -> String {
    let s = realm.scope().unwrap();
    match s.eval(src, Some("test.js"), 1) {
        Ok(v) => v.to_string().unwrap(),
        Err(e) => format!("THROW {}", e.details(&s).text()),
    }
}

#[test]
fn media_source_is_closed_and_its_object_url_is_unfetchable() {
    let e = env("<!doctype html><title>t</title>");
    let r = &e.main;
    assert_eq!(
        eval(r, "const ms = new MediaSource(); [ms instanceof EventTarget, ms.readyState, ms.duration, ms.sourceBuffers.length, MediaSource.isTypeSupported('video/mp4'), typeof ms.onsourceopen].join()"),
        "true,closed,NaN,0,true,object"
    );
    assert_eq!(eval(r, "try { ms.addSourceBuffer('video/mp4'); 'no' } catch (e) { e.name }"), "InvalidStateError");
    assert_eq!(eval(r, r#"MediaSource.isTypeSupported('video/mp4; codecs="unknown"')"#), "false");
    assert_eq!(
        eval(r, r#"try { ms.addSourceBuffer('video/mp4; codecs="unknown"'); 'no' } catch (e) { e.name }"#),
        "NotSupportedError"
    );
    assert_eq!(eval(r, "try { ms.endOfStream(); 'no' } catch (e) { e.name }"), "InvalidStateError");
    assert_eq!(
        eval(
            r,
            "const u = URL.createObjectURL(ms); [u.startsWith('blob:http://localhost:5173/'), u.length > 30].join()"
        ),
        "true,true"
    );
    // Not a blob entry: fetch and XHR resolve nothing for it.
    let url = eval(r, "u");
    assert!(e.page.blob_url(&url).is_none());
    assert_eq!(eval(r, "try { URL.createObjectURL({}); 'no' } catch (e) { e.name }"), "TypeError");
}

#[cfg(feature = "test-hooks")]
#[test]
fn attached_aac_seek_presents_decoded_tone() {
    assert_attached_tone(r#"audio/mp4; codecs="mp4a.40.2""#);
}

#[cfg(feature = "test-hooks")]
#[test]
fn attached_aac_seek_presents_decoded_tone_with_unspecified_codecs() {
    assert_attached_tone("audio/mp4");
}

/// A browser thread's task queue: media element tasks, MediaSource events and
/// SourceBuffer updates run when the test drains it, as between page tasks.
#[cfg(feature = "test-hooks")]
#[derive(Default)]
struct Tasks(RefCell<std::collections::VecDeque<Box<dyn FnOnce()>>>);
#[cfg(feature = "test-hooks")]
impl Tasks {
    fn drain(&self) {
        for _ in 0..10_000 {
            let task = self.0.borrow_mut().pop_front();
            let Some(task) = task else { return };
            task();
        }
        panic!("task loop did not settle");
    }
}
#[cfg(feature = "test-hooks")]
struct Hooks(Rc<Tasks>);
#[cfg(feature = "test-hooks")]
impl PageHooks for Hooks {
    fn post_task(&self, task: Box<dyn FnOnce()>) {
        self.0 .0.borrow_mut().push_back(task);
    }
    fn navigate(&self, _: NavigationRequest) {}
}

#[cfg(feature = "test-hooks")]
fn assert_attached_tone(mime: &str) {
    let e = env("<audio id='audio'></audio>");
    let tasks = Rc::new(Tasks::default());
    e.page.set_hooks(Some(Rc::new(Hooks(tasks.clone()))));
    let scope = e.main.scope().unwrap();
    scope.global().set_atom("audioMime", scope.string(mime)).unwrap();
    let bytes = include_bytes!("../../pyro-media/tests/fixtures/fragmented-tone.mp4");
    let bytes = bytes.iter().map(u8::to_string).collect::<Vec<_>>().join(",");
    assert_eq!(
        eval(&e.main, &format!("const tone = Uint8Array.of({bytes}).buffer; tone.byteLength")),
        include_bytes!("../../pyro-media/tests/fixtures/fragmented-tone.mp4").len().to_string()
    );
    // Resource selection runs in a task: the source opens there, and its URL
    // stays registered until the sourceopen listener revokes it.
    assert_eq!(
        eval(
            &e.main,
            r#"
        const audio = document.getElementById("audio");
        const source = new MediaSource();
        const url = URL.createObjectURL(source);
        const seen = [];
        source.addEventListener("sourceopen", () => {
            seen.push("sourceopen");
            URL.revokeObjectURL(url);
        }, { once: true });
        audio.addEventListener("seeked", () => seen.push("seeked"));
        audio.src = url;
        audio.load();
        source.readyState
    "#
        ),
        "closed"
    );
    tasks.drain();
    assert_eq!(
        eval(
            &e.main,
            r#"
        const buffer = source.addSourceBuffer(audioMime);
        buffer.addEventListener("updateend", () => seen.push("updateend"));
        buffer.appendBuffer(tone);
        [seen.join(), source.readyState, buffer.updating].join("|")
    "#
        ),
        "sourceopen|open|true"
    );
    tasks.drain();
    assert_eq!(
        eval(&e.main, r#"source.endOfStream(); [seen.join(), buffer.updating, source.readyState].join("|")"#),
        "sourceopen,updateend|false|ended"
    );
    tasks.drain();
    assert_eq!(eval(&e.main, "audio.currentTime = 0.4; String(audio.seeking)"), "true");
    tasks.drain();
    assert_eq!(
        eval(&e.main, r#"[seen.join(), audio.seeking, audio.readyState].join("|")"#),
        "sourceopen,updateend,seeked|false|4"
    );
    let node = e.page.dom.borrow().get_element_by_id(e.page.document(), "audio").unwrap().unwrap();
    let (time, rate, pcm) = e.page.media_audio_channel(node).expect("attached decoder output");
    assert!((time - 0.4).abs() < 0.025);
    assert_eq!(rate, 48000);
    assert_eq!(pcm.len(), 1024);
    let rms = (pcm.iter().map(|x| x * x).sum::<f32>() / pcm.len() as f32).sqrt();
    assert!((0.06..0.12).contains(&rms), "440 Hz fixture RMS {rms}");
    let crossings = pcm.windows(2).filter(|w| w[0] < 0. && w[1] >= 0.).count();
    assert!((8..=10).contains(&crossings), "440 Hz fixture crossings {crossings}");
}
