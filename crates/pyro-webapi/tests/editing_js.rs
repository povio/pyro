//! Editing from JavaScript and input on the real JavaScriptCore: clipboard
//! events and shortcuts, execCommand and queryCommand*, word/line caret
//! movement and deletion (keys and Playwright's macOS `commands`),
//! Selection.modify, Shift+Enter, undo/redo, double/triple click and
//! caretRangeFromPoint/caretPositionFromPoint.

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

const MAC: bool = cfg!(target_os = "macos");

fn mods(shift: bool, ctrl: bool, alt: bool, meta: bool) -> input::Modifiers {
    input::Modifiers { shift, ctrl, alt, meta }
}

/// The platform's shortcut modifier (Meta on macOS, else Control).
fn primary(shift: bool) -> input::Modifiers {
    mods(shift, !MAC, false, MAC)
}

/// The platform's word modifier (Alt on macOS, else Control).
fn word(shift: bool) -> input::Modifiers {
    mods(shift, !MAC, MAC, false)
}

fn press(page: &Rc<PageDom>, key: &str, text: &str, modifiers: input::Modifiers) {
    let k = input::KeyInput { key: key.into(), code: key.into(), text: text.into(), modifiers, ..Default::default() };
    input::dispatch_key(page, if text.is_empty() { input::KeyKind::RawKeyDown } else { input::KeyKind::KeyDown }, &k);
    input::dispatch_key(page, input::KeyKind::KeyUp, &k);
}

fn command(page: &Rc<PageDom>, key: &str, modifiers: input::Modifiers, commands: &[&str]) {
    let k = input::KeyInput {
        key: key.into(),
        code: key.into(),
        modifiers,
        commands: commands.iter().map(|c| c.to_string()).collect(),
        ..Default::default()
    };
    input::dispatch_key(page, input::KeyKind::RawKeyDown, &k);
    input::dispatch_key(page, input::KeyKind::KeyUp, &k);
}

fn click(page: &Rc<PageDom>, x: f64, y: f64, count: u32) {
    let m = input::MouseInput {
        x,
        y,
        button: input::MouseButton::Left,
        buttons: 1,
        click_count: count,
        ..Default::default()
    };
    input::dispatch_mouse(page, input::MouseKind::Pressed, &m).unwrap();
    input::dispatch_mouse(page, input::MouseKind::Released, &input::MouseInput { buttons: 0, ..m }).unwrap();
}

/// A point inside the `index`th character of `selector`'s first text node.
fn char_point(r: &Realm, selector: &str, index: u32) -> (f64, f64) {
    let v = eval(
        r,
        &format!(
            "(() => {{ const t = document.querySelector('{selector}').firstChild; const g = document.createRange(); \
             g.setStart(t, {index}); g.setEnd(t, {index} + 1); const b = g.getBoundingClientRect(); \
             return [b.x + b.width / 2, b.y + b.height / 2].join(); }})()"
        ),
    );
    let (x, y) = v.split_once(',').unwrap();
    (x.parse().unwrap(), y.parse().unwrap())
}

const SEL: &str = "window.sel = () => { const s = getSelection(); const n = x => x ? (x.nodeType === 3 ? '\"' + x.data + '\"' : x.nodeName) : null; \
    return [n(s.anchorNode), s.anchorOffset, n(s.focusNode), s.focusOffset].join(); }; 1";

#[test]
fn clipboard_event_interface_is_lazy_and_constructible() {
    let e = env("<body></body>");
    let r = &e.main;
    assert_eq!(
        eval(r, "typeof ClipboardEvent + ':' + ClipboardEvent.length + ':' + ClipboardEvent.name"),
        "function:1:ClipboardEvent"
    );
    assert_eq!(
        eval(
            r,
            "const dt = new DataTransfer(); dt.setData('text/plain', 'hi'); \
             const ev = new ClipboardEvent('paste', { clipboardData: dt, bubbles: true, cancelable: true }); \
             [ev instanceof ClipboardEvent, ev instanceof Event, ev.clipboardData === dt, ev.clipboardData.getData('text'), \
              ev.bubbles, ev.isTrusted, Object.prototype.toString.call(ev), new ClipboardEvent('copy').clipboardData].join()"
        ),
        "true,true,true,hi,true,false,[object ClipboardEvent],"
    );
    assert!(eval(r, "new ClipboardEvent('x', { clipboardData: {} })").starts_with("THROW TypeError"));
    assert!(eval(r, "new ClipboardEvent()").starts_with("THROW TypeError"));
    assert!(eval(
        r,
        "Object.getOwnPropertyDescriptor(ClipboardEvent.prototype, 'clipboardData').get.call(new Event('x'))"
    )
    .starts_with("THROW TypeError"));
    // A synthetic paste reaches listeners with its data and does nothing else.
    assert_eq!(
        eval(r, "let got = ''; document.addEventListener('paste', e => got = e.clipboardData.getData('text/plain')); document.body.dispatchEvent(ev); got"),
        "hi"
    );
}

/// The WPT data_transfer_on_input_event_with_insertfrompaste_type flow:
/// copy from a host by shortcut, paste into a host and a textarea.
#[test]
fn shortcuts_copy_and_paste_with_clipboard_events() {
    let e = env("<body><div id=copy contenteditable=true>copyMe</div><div contenteditable=true id=paste></div><textarea id=paste2></textarea></body>");
    let r = &e.main;
    eval(
        r,
        "window.log = []; for (const t of ['copy', 'paste', 'beforeinput', 'input']) document.addEventListener(t, ev => log.push([t, ev.target.id, ev.isTrusted, ev.composed, \
           ev.constructor.name, ev.inputType || '', ev.data ?? 'null', ev.clipboardData ? ev.clipboardData.getData('text') : (ev.dataTransfer ? 'dt:' + ev.dataTransfer.getData('text') : '-')].join(':')));",
    );
    eval(r, "const range = document.createRange(); range.selectNodeContents(copy); getSelection().removeAllRanges(); getSelection().addRange(range); copy.focus()");
    eval(r, "range.selectNodeContents(copy); getSelection().removeAllRanges(); getSelection().addRange(range);");
    press(&e.page, "c", "", primary(false));
    assert_eq!(e.page.context_services().clipboard_text().as_deref(), Some("copyMe"));
    eval(r, "getSelection().removeAllRanges(); paste.focus()");
    press(&e.page, "v", "", primary(false));
    eval(r, "paste2.focus()");
    press(&e.page, "v", "", primary(false));
    assert_eq!(
        eval(r, "log.join(' | ')"),
        "copy:copy:true:true:ClipboardEvent::null: | \
         paste:paste:true:true:ClipboardEvent::null:copyMe | \
         beforeinput:paste:true:true:InputEvent:insertFromPaste:null:dt:copyMe | \
         input:paste:true:true:InputEvent:insertFromPaste:null:dt:copyMe | \
         paste:paste2:true:true:ClipboardEvent::null:copyMe | \
         beforeinput:paste2:true:true:InputEvent:insertFromPaste:copyMe:- | \
         input:paste2:true:true:InputEvent:insertFromPaste:copyMe:-"
    );
    assert_eq!(eval(r, "paste.innerHTML + '|' + paste2.value + '|' + paste2.selectionStart"), "copyMe|copyMe|6");
    // navigator.clipboard sees what was copied (with clipboard-read).
    e.page.context_services().grant(None, &["clipboard-read".into(), "clipboard-write".into()]);
    eval(r, "window.read = null; navigator.clipboard.readText().then(t => read = t)");
    assert_eq!(eval(r, "read"), "copyMe");
}

/// Pasted markup loses scripts, event handler attributes and `javascript:`
/// URLs (the clipboard may hold markup another page wrote); insertHTML keeps
/// the attributes.
#[test]
fn rich_paste_is_sanitized() {
    let e = env(
        "<body><div id=src><b onclick='hit()' ONMOUSEOVER='hit()' €x=1 title=t>x</b><a href=' javascript:hit()'>l</a><a href='/ok'>k</a></div>\
         <div id=ed contenteditable></div></body>",
    );
    let r = &e.main;
    eval(r, "getSelection().selectAllChildren(src)");
    press(&e.page, "c", "", primary(false));
    eval(r, "ed.focus()");
    press(&e.page, "v", "", primary(false));
    assert_eq!(eval(r, "ed.innerHTML"), "<b €x=\"1\" title=\"t\">x</b><a>l</a><a href=\"/ok\">k</a>");
    eval(r, "ed.textContent = ''; ed.focus(); document.execCommand('insertHTML', false, '<i onclick=\"f()\">y</i>')");
    assert_eq!(eval(r, "ed.innerHTML"), "<i onclick=\"f()\">y</i>");
}

#[test]
fn copy_handlers_set_the_data_and_cut_deletes() {
    let e = env("<body><input id=i value='hello world'><div id=ed contenteditable>abc def</div></body>");
    let r = &e.main;
    eval(r, "i.focus(); i.setSelectionRange(0, 5)");
    // A canceled copy writes what the handler set.
    eval(r, "const h = ev => { ev.clipboardData.setData('text/plain', 'custom'); ev.preventDefault(); }; document.addEventListener('copy', h)");
    press(&e.page, "c", "", primary(false));
    assert_eq!(e.page.context_services().clipboard_text().as_deref(), Some("custom"));
    eval(r, "document.removeEventListener('copy', h)");
    // execCommand('copy') copies the control's selection.
    assert_eq!(eval(r, "document.execCommand('copy')"), "true");
    assert_eq!(e.page.context_services().clipboard_text().as_deref(), Some("hello"));
    // Cut: copies, then deletes with deleteByCut.
    eval(
        r,
        "window.types = []; i.addEventListener('input', ev => types.push(ev.inputType)); i.setSelectionRange(5, 11)",
    );
    press(&e.page, "x", "", primary(false));
    assert_eq!(eval(r, "i.value + '|' + types.join()"), "hello|deleteByCut");
    assert_eq!(e.page.context_services().clipboard_text().as_deref(), Some(" world"));
    // Paste is not available to script; a shortcut pastes.
    assert_eq!(eval(r, "document.execCommand('paste') + ':' + document.queryCommandSupported('paste')"), "false:false");
    press(&e.page, "v", "", primary(false));
    assert_eq!(eval(r, "i.value + '|' + types.join()"), "hello world|deleteByCut,insertFromPaste");
    // Cut in editable content.
    eval(r, "ed.focus(); getSelection().setBaseAndExtent(ed.firstChild, 3, ed.firstChild, 7)");
    press(&e.page, "x", "", primary(false));
    assert_eq!(eval(r, "ed.innerHTML"), "abc");
    assert_eq!(e.page.context_services().clipboard_text().as_deref(), Some(" def"));
    // A canceled paste inserts nothing.
    eval(r, "document.addEventListener('paste', ev => ev.preventDefault())");
    press(&e.page, "v", "", primary(false));
    assert_eq!(eval(r, "ed.innerHTML"), "abc");
}

#[test]
fn removing_bold_preserves_editability_boundaries() {
    let e = env("<body><div id=ed contenteditable></div></body>");
    for (mode, inside, expected) in [
        ("false", "<b>e</b>", "<b>e</b>"),
        ("false", "<span contenteditable><b>e</b></span>", "<span contenteditable=\"\">e</span>"),
        ("plaintext-only", "<b>e</b>", "e"),
    ] {
        let script = format!(r#"
            (() => {{
                ed.innerHTML = 'abc<b>d<span contenteditable="{mode}">{inside}</span>f</b>ghi';
                ed.focus();
                const carrier = ed.firstElementChild, island = carrier.firstElementChild;
                getSelection().setBaseAndExtent(carrier.lastChild, 1, carrier.firstChild, 0);
                document.execCommand('bold');
                return [ed.firstElementChild === island, island.innerHTML, getSelection().direction].join('|');
            }})()
        "#);
        assert_eq!(eval(&e.main, &script), format!("true|{expected}|backward"));
    }
    let partial = r#"
        ed.innerHTML = '<b>left<span contenteditable="false"><i contenteditable="true"><b>inner</b></i></span>right</b>';
        ed.focus();
        const carrier = ed.firstChild, island = carrier.firstElementChild;
        getSelection().setBaseAndExtent(carrier.lastChild, 2, carrier.firstChild, 2);
        document.execCommand('bold');
        [ed.innerHTML, ed.contains(island), getSelection().direction, document.queryCommandIndeterm('bold')].join('|')
    "#;
    assert_eq!(eval(&e.main, partial), r#"<b>le</b>ft<span contenteditable="false"><i contenteditable="true">inner</i></span>ri<b>ght</b>|true|backward|false"#);

}

#[test]
fn inline_formatting_skips_noneditable_islands() {
    let e = env("<body><div id=ed contenteditable>ab<span id=island contenteditable=false>LOCK</span>cd</div></body>");
    let r = &e.main;
    eval(r, "ed.focus(); const original = island; const child = original.firstChild; const s = getSelection(); s.setBaseAndExtent(ed.lastChild,1,ed.firstChild,1);");
    assert_eq!(eval(r, "document.execCommand('bold')"), "true");
    assert_eq!(eval(r, "ed.innerHTML"), "a<b>b</b><span id=\"island\" contenteditable=\"false\">LOCK</span><b>c</b>d");
    assert_eq!(eval(r, "[s.toString(),s.direction,document.queryCommandState('bold'),document.queryCommandIndeterm('bold')].join()"), "bLOCKc,backward,true,false");
    assert_eq!(eval(r, "document.execCommand('bold'); [ed.innerHTML,original===island,original.firstChild===child].join('|')"), "ab<span id=\"island\" contenteditable=\"false\">LOCK</span>cd|true|true");
}

#[test]
fn exec_command_formatting_and_structure() {
    let e = env("<body><div id=ed contenteditable><p id=p>one two three</p></div></body>");
    let r = &e.main;
    eval(r, SEL);
    eval(r, "window.types = []; ed.addEventListener('input', ev => types.push(ev.inputType)); ed.focus(); const t = p.firstChild; getSelection().setBaseAndExtent(t, 4, t, 7)");
    assert_eq!(
        eval(r, "document.queryCommandState('bold') + ':' + document.queryCommandSupported('bold')"),
        "false:true"
    );
    assert_eq!(eval(r, "document.execCommand('bold')"), "true");
    assert_eq!(
        eval(r, "p.innerHTML + '|' + getSelection().toString() + '|' + document.queryCommandState('bold')"),
        "one <b>two</b> three|two|true"
    );
    assert_eq!(
        eval(r, "document.execCommand('bold'); p.innerHTML + '|' + document.queryCommandState('bold')"),
        "one two three|false"
    );
    assert_eq!(eval(r, "document.execCommand('italic'); p.innerHTML"), "one <i>two</i> three");
    // Part of the selection italic: indeterminate.
    assert_eq!(
        eval(r, "getSelection().setBaseAndExtent(p.firstChild, 0, p.lastChild, 3); [document.queryCommandState('italic'), document.queryCommandIndeterm('italic')].join()"),
        "false,true"
    );
    assert_eq!(eval(r, "document.execCommand('removeFormat'); p.innerHTML"), "one two three");
    // Links.
    assert_eq!(
        eval(r, "getSelection().setBaseAndExtent(p.firstChild, 0, p.firstChild, 3); document.execCommand('createLink', false, 'https://x.test/'); p.innerHTML"),
        "<a href=\"https://x.test/\">one</a> two three"
    );
    assert_eq!(eval(r, "document.execCommand('unlink'); p.innerHTML"), "one two three");
    // formatBlock and its value.
    assert_eq!(eval(r, "document.execCommand('formatBlock', false, '<h2>'); ed.innerHTML"), "<h2>one two three</h2>");
    assert_eq!(eval(r, "document.queryCommandValue('formatBlock')"), "h2");
    // Lists toggle.
    assert_eq!(eval(r, "document.execCommand('insertUnorderedList'); ed.innerHTML"), "<ul><li>one two three</li></ul>");
    assert_eq!(eval(r, "document.queryCommandState('insertUnorderedList')"), "true");
    assert_eq!(eval(r, "document.execCommand('insertUnorderedList'); ed.innerHTML"), "<div>one two three</div>");
    // insertHTML replaces the selection; the caret goes after it.
    assert_eq!(
        eval(r, "ed.normalize(); const d = ed.firstChild.firstChild; getSelection().setBaseAndExtent(d, 4, d, 7); document.execCommand('insertHTML', false, '<em>2</em><script>x()</script>'); ed.innerHTML"),
        "<div>one <em>2</em> three</div>"
    );
    // insertLineBreak at the end of a line adds the placeholder.
    assert_eq!(
        eval(r, "const last = ed.firstChild.lastChild; getSelection().collapse(last, last.length); document.execCommand('insertLineBreak'); ed.innerHTML + '|' + sel()"),
        "<div>one <em>2</em> three<br><br></div>|DIV,4,DIV,4"
    );
    assert_eq!(
        eval(r, "types.join()"),
        "formatBold,formatBold,formatItalic,formatRemove,insertLink,,formatBlock,insertUnorderedList,insertUnorderedList,insertHTML,insertLineBreak"
    );
    // Typing on the new line drops the placeholder.
    assert_eq!(
        eval(r, "document.execCommand('insertText', false, 'x'); ed.innerHTML"),
        "<div>one <em>2</em> three<br>x</div>"
    );
    // Query methods on unsupported commands.
    assert_eq!(
        eval(r, "[document.queryCommandSupported('nope'), document.queryCommandEnabled('nope'), document.queryCommandValue('nope'), document.execCommand('nope'), document.queryCommandEnabled('bold'), document.queryCommandSupported('insertText')].join()"),
        "false,false,,false,true,true"
    );
    // Outside editable content formatting is disabled and does nothing.
    assert_eq!(eval(r, "ed.blur(); getSelection().removeAllRanges(); [document.queryCommandEnabled('bold'), document.execCommand('bold')].join()"), "false,false");
}

#[test]
fn control_history_remembers_both_selections_across_an_intervening_move() {
    let e = env("<body><textarea id=t>abcdef</textarea></body>");
    let r = &e.main;
    eval(r, "t.focus(); t.setSelectionRange(1,4,'backward'); document.execCommand('insertText',false,'Z'); t.setSelectionRange(0,1,'forward')");
    assert_eq!(eval(r, "document.execCommand('undo'); [t.value,t.selectionStart,t.selectionEnd,t.selectionDirection].join()"), "abcdef,1,4,backward");
    assert_eq!(eval(r, "document.execCommand('redo'); [t.value,t.selectionStart,t.selectionEnd,t.selectionDirection].join()"), "aZef,2,2,forward");
}

#[test]
fn undo_restores_anchor_focus_and_text_boundary_affinity() {
    let e = env("<body><div id=ed contenteditable>a😀<i>bc</i>def</div></body>");
    let r = &e.main;
    eval(r, "ed.focus(); const sel=getSelection(); sel.setBaseAndExtent(ed.lastChild,1,ed.firstChild,3);");
    assert_eq!(eval(r, "sel.direction + '|' + sel.toString()"), "backward|bcd");
    eval(r, "document.execCommand('insertText',false,'X'); const changed=ed.innerHTML; document.execCommand('undo')");
    assert_eq!(eval(r, "[sel.direction,sel.anchorNode===ed.lastChild,sel.anchorOffset,sel.focusNode===ed.querySelector('i').firstChild,sel.focusOffset,sel.toString()].join('|')"),
        "backward|true|1|true|0|bcd");
    assert_eq!(eval(r, "document.execCommand('redo'); [ed.innerHTML===changed,sel.isCollapsed].join()"), "true,true");
    eval(r, "document.execCommand('undo'); sel.collapse(ed.firstChild,3); document.execCommand('insertText',false,'Y'); document.execCommand('undo')");
    assert_eq!(eval(r, "[sel.isCollapsed,sel.anchorNode===ed.firstChild,sel.focusNode===ed.firstChild,sel.focusOffset].join()"), "true,true,true,3");
}

#[test]
fn undo_and_redo_restore_text_controls_and_hosts() {
    let e = env("<body><input id=i><div id=ed contenteditable><p>ab</p></div></body>");
    let r = &e.main;
    eval(r, "window.types = []; document.addEventListener('input', ev => types.push(ev.inputType)); i.focus()");
    for c in ["h", "e", "y"] {
        press(&e.page, c, c, input::Modifiers::default());
    }
    press(&e.page, "Backspace", "", input::Modifiers::default());
    assert_eq!(eval(r, "i.value"), "he");
    // The deletion, then the typing (one step).
    press(&e.page, "z", "", primary(false));
    assert_eq!(eval(r, "i.value"), "hey");
    assert_eq!(
        eval(r, "document.execCommand('undo') + ':' + i.value + ':' + document.execCommand('undo')"),
        "true::false"
    );
    press(&e.page, "z", "", primary(true));
    assert_eq!(eval(r, "i.value + ':' + document.queryCommandEnabled('redo')"), "hey:true");
    assert_eq!(
        eval(r, "types.join()"),
        "insertText,insertText,insertText,deleteContentBackward,historyUndo,historyUndo,historyRedo"
    );
    // A host: typing and Enter, undone.
    eval(r, "ed.focus(); getSelection().collapse(ed.querySelector('p').firstChild, 2)");
    press(&e.page, "c", "c", input::Modifiers::default());
    press(&e.page, "Enter", "\r", input::Modifiers::default());
    assert_eq!(eval(r, "ed.innerHTML"), "<p>abc</p><p><br></p>");
    assert_eq!(eval(r, "document.execCommand('undo'); ed.innerHTML"), "<p>abc</p>");
    assert_eq!(eval(r, "document.execCommand('undo'); ed.innerHTML + ':' + getSelection().focusOffset"), "<p>ab</p>:2");
    assert_eq!(eval(r, "document.execCommand('redo'); ed.innerHTML"), "<p>abc</p>");
    // A canceled historyUndo leaves the content.
    eval(r, "ed.addEventListener('beforeinput', ev => { if (ev.inputType === 'historyUndo') ev.preventDefault(); })");
    assert_eq!(eval(r, "document.execCommand('undo'); ed.innerHTML"), "<p>abc</p>");
}

#[test]
fn word_and_line_keys_in_text_controls() {
    let e = env("<body><input id=i value='one two three'><textarea id=ta></textarea></body>");
    let r = &e.main;
    eval(r, "i.focus(); i.setSelectionRange(13, 13); window.types = []; i.addEventListener('beforeinput', ev => types.push(ev.inputType))");
    let pos = |id: &str| eval(r, &format!("{id}.selectionStart + ',' + {id}.selectionEnd"));
    press(&e.page, "ArrowLeft", "", word(false));
    assert_eq!(pos("i"), "8,8");
    press(&e.page, "ArrowLeft", "", word(true));
    assert_eq!(pos("i"), "4,8");
    assert_eq!(eval(r, "i.selectionDirection"), "backward");
    press(&e.page, "ArrowRight", "", word(false));
    assert_eq!(pos("i"), "13,13");
    press(&e.page, "Backspace", "", word(false));
    assert_eq!(eval(r, "i.value + '|' + types.join()"), "one two |deleteWordBackward");
    eval(r, "i.setSelectionRange(0, 0)");
    press(&e.page, "Delete", "", word(false));
    assert_eq!(eval(r, "i.value + '|' + types.join()"), " two |deleteWordBackward,deleteWordForward");
    // Playwright's macOS commands run in place of the key's own action.
    eval(r, "i.value = 'alpha beta'; i.setSelectionRange(10, 10)");
    command(&e.page, "Backspace", mods(false, false, true, false), &["deleteWordBackward"]);
    assert_eq!(eval(r, "i.value"), "alpha ");
    command(&e.page, "a", mods(false, false, false, true), &["selectAll"]);
    assert_eq!(pos("i"), "0,6");
    command(&e.page, "ArrowLeft", mods(false, false, false, true), &["moveToLeftEndOfLine"]);
    assert_eq!(pos("i"), "0,0");
    // Unhandled commands leave the key's default action.
    command(&e.page, "End", input::Modifiers::default(), &["scrollToEndOfDocument"]);
    assert_eq!(pos("i"), "6,6");
    // Textarea lines.
    eval(r, "ta.focus(); ta.value = 'ab\\ncd ef\\ngh'; ta.setSelectionRange(5, 5)");
    press(&e.page, "Home", "", input::Modifiers::default());
    assert_eq!(pos("ta"), "3,3");
    press(&e.page, "End", "", mods(true, false, false, false));
    assert_eq!(pos("ta"), "3,8");
    command(&e.page, "Backspace", mods(false, false, false, true), &["deleteToBeginningOfLine"]);
    assert_eq!(eval(r, "JSON.stringify(ta.value)"), r#""ab\n\ngh""#);
}

#[test]
fn deletion_at_text_edge_preserves_locked_blocks() {
    let e = env("<body><div id=ed contenteditable><p>abc</p><pre id=locked contenteditable=false>keep</pre></div></body>");
    let r = &e.main;
    eval(r, "ed.focus(); const original = locked; const left = ed.firstChild.firstChild; getSelection().collapse(left,3)");
    press(&e.page, "Delete", "", input::Modifiers::default());
    assert_eq!(eval(r, "[original.parentNode===ed,getSelection().focusNode===left,getSelection().focusOffset,document.queryCommandEnabled('undo')].join()"), "true,true,3,false");
    eval(r, "ed.prepend(original); getSelection().collapse(left,0)");
    press(&e.page, "Backspace", "", input::Modifiers::default());
    assert_eq!(eval(r, "[original.parentNode===ed,getSelection().focusNode===left,getSelection().focusOffset].join()"), "true,true,0");
    // A second editable text run provides a position across the locked node.
    eval(r, "ed.prepend(document.createTextNode('far')); getSelection().collapse(left,0)");
    press(&e.page, "Backspace", "", input::Modifiers::default());
    assert_eq!(eval(r, "[original.isConnected,ed.textContent].join()"), "false,farabc");
    eval(r, "original.style.display='inline'; ed.append(original); getSelection().collapse(left,3)");
    press(&e.page, "Delete", "", input::Modifiers::default());
    assert_eq!(eval(r, "[original.isConnected,ed.textContent].join()"), "false,farabc");
}

#[test]
fn nested_paragraph_join_preserves_the_destination_and_undo() {
    let e = env("<body><div id=ed contenteditable><div><div>a&nbsp;&nbsp;&nbsp;</div>&nbsp;&nbsp;&nbsp;b</div></div>");
    let r = &e.main;
    eval(r, "ed.focus(); const outer=ed.firstChild, inner=outer.firstChild, text=inner.firstChild; getSelection().collapse(text,4)");
    press(&e.page, "Delete", "", input::Modifiers::default());
    assert_eq!(eval(r, "ed.innerHTML"), "<div>a&nbsp; &nbsp; &nbsp; b</div>");
    assert_eq!(eval(r, "[ed.firstChild===inner, getSelection().focusNode===text, getSelection().focusOffset].join()"), "true,true,4");
    assert_eq!(eval(r, "document.execCommand('undo'); ed.innerHTML"), "<div><div>a&nbsp;&nbsp;&nbsp;</div>&nbsp;&nbsp;&nbsp;b</div>");
}

#[test]
fn paragraph_insertion_leaves_select_subtrees_intact() {
    let e = env("<body><div id=ed contenteditable><div>L<select><optgroup><option>abc</option><option selected>def</option></optgroup></select>R</div></div></body>");
    let r = &e.main;
    eval(r, "ed.focus(); const original = ed.querySelector('select'); const option = original.firstChild.firstChild; getSelection().collapse(option.firstChild,2); document.execCommand('insertParagraph')");
    assert_eq!(eval(r, "ed.innerHTML"), r#"<div><br></div><div>L<select><optgroup><option>abc</option><option selected="">def</option></optgroup></select>R</div>"#);
    assert_eq!(eval(r, "[ed.querySelector('select')===original,original.firstChild.firstChild===option,original.selectedIndex,getSelection().focusNode===ed.lastChild.firstChild,getSelection().focusOffset].join()"), "true,true,1,true,0");
}

#[test]
fn paragraph_insertion_in_a_phrasing_host_uses_the_caret_style() {
    let e = env("<body><span id=ed contenteditable style='display:block;white-space:pre'>ab</span>outside</body>");
    let r = &e.main;
    eval(r, "ed.focus(); getSelection().collapse(ed.firstChild,1); document.execCommand('insertParagraph')");
    assert_eq!(eval(r, "ed.innerHTML"), "a\nb");
    assert_eq!(eval(r, "[ed.childNodes.length,ed.childNodes[1].data,getSelection().focusNode===ed.lastChild,getSelection().focusOffset].join('|')"), "3|\n|true|0");
    eval(r, "ed.innerHTML='<span style=\"display:block\">ab</span>'; getSelection().collapse(ed.firstChild.firstChild,1); document.execCommand('insertParagraph')");
    assert_eq!(eval(r, "ed.innerHTML"), "<span style=\"display:block\">a\nb</span>");
    eval(r, "const blockHost=document.createElement('div'); document.body.append(blockHost); blockHost.contentEditable='true'; blockHost.innerHTML='<span style=\"display:block\">ab</span>'; blockHost.focus(); getSelection().collapse(blockHost.firstChild.firstChild,1); document.execCommand('insertParagraph')");
    assert_eq!(eval(r, "blockHost.innerHTML"), r#"<span style="display:block">a</span><span style="display:block">b</span>"#);
    eval(r, "ed.focus()");
    eval(r, "ed.innerHTML='<div style=\"display:inline;white-space:normal\">ab</div>'; getSelection().collapse(ed.firstChild.firstChild,1); document.execCommand('insertText',false,'\\n')");
    assert_eq!(eval(r, "ed.innerHTML"), r#"<div style="display:inline;white-space:normal">a<br>b</div>"#);
    assert_eq!(eval(r, "ed.nextSibling.data"), "outside");
}

#[test]
fn fresh_typing_at_link_edges_does_not_extend_the_link() {
    let e = env("<body><div id=ed contenteditable><p>L<a id=link href='/go'>abc</a>R</p></div></body>");
    let r = &e.main;
    eval(r, "ed.focus(); const original = link; getSelection().collapse(link.firstChild,3); document.execCommand('insertText',false,'XY')");
    assert_eq!(eval(r, "[ed.textContent,link.textContent,original===link,getSelection().focusOffset].join()"), "LabcXYR,abc,true,2");
    eval(r, "getSelection().collapse(link.firstChild,0); document.execCommand('insertText',false,'Z')");
    assert_eq!(eval(r, "ed.textContent"), "LZabcXYR");
    eval(r, "getSelection().collapse(link.firstChild,1); document.execCommand('insertText',false,'Q')");
    assert_eq!(eval(r, "link.textContent"), "aQbc");
}

#[test]
fn contenteditable_line_breaks_arrows_and_words() {
    let e = env("<body><div id=ed contenteditable><p>hello world</p></div></body>");
    let r = &e.main;
    eval(r, SEL);
    eval(r, "window.types = []; ed.addEventListener('beforeinput', ev => types.push(ev.inputType)); ed.focus(); const t = ed.querySelector('p').firstChild; getSelection().collapse(t, 5)");
    press(&e.page, "Enter", "\r", mods(true, false, false, false));
    assert_eq!(
        eval(r, "ed.innerHTML + '|' + sel() + '|' + types.join()"),
        "<p>hello<br>&nbsp;world</p>|\"\u{a0}world\",0,\"\u{a0}world\",0|insertLineBreak"
    );
    // ArrowUp keeps the column on the line above; ArrowDown comes back.
    press(&e.page, "ArrowRight", "", input::Modifiers::default());
    press(&e.page, "ArrowRight", "", input::Modifiers::default());
    press(&e.page, "ArrowUp", "", input::Modifiers::default());
    assert_eq!(eval(r, "sel()"), r#""hello",2,"hello",2"#);
    press(&e.page, "ArrowDown", "", input::Modifiers::default());
    assert_eq!(eval(r, "sel()"), "\"\u{a0}world\",2,\"\u{a0}world\",2");
    press(&e.page, "End", "", input::Modifiers::default());
    assert_eq!(eval(r, "sel()"), "\"\u{a0}world\",6,\"\u{a0}world\",6");
    press(&e.page, "Backspace", "", word(false));
    assert_eq!(eval(r, "ed.innerHTML + '|' + types.join()"), "<p>hello<br>&nbsp;</p>|insertLineBreak,deleteWordBackward");
    press(&e.page, "Home", "", input::Modifiers::default());
    press(&e.page, "ArrowUp", "", input::Modifiers::default());
    press(&e.page, "ArrowRight", "", word(true));
    assert_eq!(eval(r, "sel()"), r#""hello",0,"hello",5"#);
}

/// Outside editable content, word keys and macOS move commands leave a
/// selection alone unless they extend it (as in Chromium).
#[test]
fn document_selection_moves_only_when_extended() {
    let e = env("<body><p id=p>one two three</p><button id=b>b</button></body>");
    let r = &e.main;
    eval(r, SEL);
    eval(r, "b.focus(); getSelection().collapse(p.firstChild, 0)");
    press(&e.page, "ArrowRight", "", word(false));
    command(&e.page, "ArrowRight", input::Modifiers::default(), &["moveWordRight"]);
    assert_eq!(eval(r, "sel()"), r#""one two three",0,"one two three",0"#);
    press(&e.page, "ArrowRight", "", word(true));
    assert_eq!(eval(r, "sel()"), r#""one two three",0,"one two three",3"#);
    command(&e.page, "ArrowRight", input::Modifiers::default(), &["moveWordRightAndModifySelection"]);
    assert_eq!(eval(r, "sel()"), r#""one two three",0,"one two three",7"#);
}

#[test]
fn selection_modify_matches_wpt() {
    let e = env(
        "<body><div>Test, these are <strong id=strong> strong </strong> <em id=em> italic </em> normal.</div>\
         <pre id=preLinefeed>\nfoo\nbar\n</pre><pre id=preBr>\nfoo<br>bar\n</pre><pre id=preLinefeedBr>\nfoo\n<br>\nbar\n</pre>\
         <div contenteditable id=target>~&gt;&gt;&gt;&gt;&gt;p+++</div><p id=para>one two</p></body>",
    );
    let r = &e.main;
    eval(r, SEL);
    let run = |src: &str| eval(r, &format!("(() => {{ const s = getSelection(); {src}; return sel(); }})()"));
    assert_eq!(
        run("s.collapse(strong.firstChild, 4); s.modify('extend', 'backward', 'word')"),
        r#"" strong ",4," strong ",1"#
    );
    assert_eq!(
        run("s.collapse(em.firstChild, 4); s.modify('extend', 'backward', 'word')"),
        r#"" italic ",4," italic ",1"#
    );
    assert_eq!(
        run("s.collapse(preLinefeed.firstChild, 3); s.modify('move', 'forward', 'character')"),
        "\"foo\nbar\n\",4,\"foo\nbar\n\",4"
    );
    assert_eq!(
        run("s.collapse(preLinefeed.firstChild, 4); s.modify('move', 'backward', 'character')"),
        "\"foo\nbar\n\",3,\"foo\nbar\n\",3"
    );
    assert_eq!(
        run("s.collapse(preBr.firstChild, 3); s.modify('move', 'forward', 'character')"),
        "\"bar\n\",0,\"bar\n\",0"
    );
    assert_eq!(
        run("s.collapse(preBr.childNodes[2], 0); s.modify('move', 'backward', 'character')"),
        r#""foo",3,"foo",3"#
    );
    assert_eq!(
        run("s.collapse(preLinefeedBr, 1); s.modify('move', 'forward', 'character')"),
        "\"\nbar\n\",0,\"\nbar\n\",0"
    );
    assert_eq!(
        run("s.collapse(preLinefeedBr, 2); s.modify('move', 'backward', 'character')"),
        "\"foo\n\",4,\"foo\n\",4"
    );
    let t = "target.firstChild";
    assert_eq!(
        run(&format!("s.collapse({t}, 6); s.modify('move', 'backward', 'word')")),
        r#""~>>>>>p+++",0,"~>>>>>p+++",0"#
    );
    assert_eq!(
        run(&format!("s.collapse({t}, 7); s.modify('move', 'forward', 'word')")),
        r#""~>>>>>p+++",10,"~>>>>>p+++",10"#
    );
    assert_eq!(
        run("s.collapse(para.firstChild, 1); s.modify('extend', 'forward', 'lineboundary')"),
        r#""one two",1,"one two",7"#
    );
    assert_eq!(run("s.modify('move', 'left', 'lineboundary')"), r#""one two",0,"one two",0"#);
    // Unknown arguments do nothing.
    assert_eq!(
        run(
            "s.modify('jump', 'forward', 'word'); s.modify('move', 'up', 'word'); s.modify('move', 'forward', 'glyph')"
        ),
        r#""one two",0,"one two",0"#
    );
}

#[test]
fn double_and_triple_click_select() {
    let e = env("<body><div id=container>hello, world</div><input id=i value='one two'></body>");
    let r = &e.main;
    eval(r, SEL);
    let (x, y) = char_point(r, "#container", 8);
    click(&e.page, x, y, 1);
    click(&e.page, x, y, 2);
    assert_eq!(eval(r, "getSelection().toString()"), "world");
    click(&e.page, x, y, 3);
    assert_eq!(eval(r, "sel()"), "DIV,0,DIV,1");
    // In an input: a triple click selects the value (typing replaces it).
    let b = eval(r, "const b = i.getBoundingClientRect(); [b.x + 5, b.y + b.height / 2].join()");
    let (ix, iy) = b.split_once(',').unwrap();
    let (ix, iy): (f64, f64) = (ix.parse().unwrap(), iy.parse().unwrap());
    click(&e.page, ix, iy, 1);
    click(&e.page, ix, iy, 2);
    click(&e.page, ix, iy, 3);
    assert_eq!(eval(r, "i.selectionStart + ',' + i.selectionEnd"), "0,7");
    press(&e.page, "x", "x", input::Modifiers::default());
    assert_eq!(eval(r, "i.value"), "x");
    // A double click selects the word at the caret.
    eval(r, "i.value = 'one two'; i.setSelectionRange(1, 1)");
    click(&e.page, ix, iy, 2);
    assert_eq!(eval(r, "i.selectionStart + ',' + i.selectionEnd"), "0,3");
}

#[test]
fn caret_from_point() {
    let e = env("<body style='margin:0'><p id=p style='margin:0'>hello world</p><div id=empty style='height:50px'></div></body>");
    let r = &e.main;
    let (x, y) = char_point(r, "#p", 6);
    let v = eval(
        r,
        &format!(
            "const g = document.caretRangeFromPoint({x}, {y}); const c = document.caretPositionFromPoint({x}, {y}); \
             [g.collapsed, g.startContainer === p.firstChild, g.startOffset, c instanceof CaretPosition, c.offsetNode === p.firstChild, c.offset, \
              typeof c.getClientRect().x, Object.prototype.toString.call(c)].join()"
        ),
    );
    assert!(
        v == "true,true,6,true,true,6,number,[object CaretPosition]"
            || v == "true,true,7,true,true,7,number,[object CaretPosition]",
        "{v}"
    );
    assert_eq!(
        eval(
            r,
            "String(document.caretPositionFromPoint(-5, -5)) + ':' + String(document.caretRangeFromPoint(5, 5000))"
        ),
        "null:null"
    );
    assert!(eval(r, "new CaretPosition()").starts_with("THROW TypeError"));
    assert!(eval(r, "document.caretPositionFromPoint(1)").starts_with("THROW TypeError"));
}

/// The selection a render snapshot carries for the renderer (`RenderSnapshot::selection`, painted
/// in screenshots): its ends moved into text, as render keys of text nodes with UTF-16 offsets;
/// text the render tree leaves out (a `<script>`'s) is passed over, a collapsed selection is none.
#[test]
fn render_snapshots_carry_the_selection_in_text() {
    let e = env("<p id=p>Some <b>bold</b> te\u{e9}xt</p><script>var x = 1</script>");
    let r = &e.main;
    let key = |script: &str| {
        let dom = e.page.dom.borrow();
        let p = dom.get_element_by_id(e.page.document(), "p").unwrap().unwrap();
        let children: Vec<_> = dom.children(p).unwrap().collect();
        let node = match script {
            "first" => children[0],
            "bold" => dom.first_child(children[1]).unwrap().unwrap(),
            _ => children[2],
        };
        pyro_dom::snapshot::render_key(node)
    };
    assert_eq!(e.page.render_view().selection, None);
    eval(r, "const range = document.createRange(); range.selectNode(document.querySelector('b')); getSelection().addRange(range)");
    assert_eq!(e.page.render_view().selection, Some(((key("bold"), 0), (key("bold"), 4))));
    // To the end of the body: its last child, a script, renders nothing.
    eval(r, "document.execCommand('selectAll')");
    assert_eq!(e.page.render_view().selection, Some(((key("first"), 0), (key("last"), 6))));
    eval(r, "getSelection().collapse(p.firstChild, 2)");
    assert_eq!(e.page.render_view().selection, None);
}
