//! Popovers, dialogs, invokers and details as a page sees them:
//! `beforetoggle`/`toggle` (`ToggleEvent`), the auto popover stack, light
//! dismiss and Escape, `popovertarget` and `command`/`commandfor`
//! (`CommandEvent`), the dialog focusing steps, `requestClose`/`closedBy`,
//! `method=dialog`, modal inertness and the top layer's hit testing
//! (html/semantics/popovers, the-dialog-element, the-details-element,
//! the-button-element/command-and-commandfor).

use std::cell::RefCell;
use std::rc::Rc;

use lp_dom::parse::{parse_document, DomRef};
use lp_dom::Dom;
use lp_js::{ClassDef, Engine, JsClass, Realm, RealmOptions};
use lp_webapi::*;

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

/// Records what the document asks to navigate to (followed links, submitted
/// forms) and runs posted tasks only when asked.
#[derive(Default)]
struct Hooks {
    navigations: RefCell<Vec<String>>,
    tasks: RefCell<Vec<Box<dyn FnOnce()>>>,
}
impl PageHooks for Hooks {
    fn post_task(&self, task: Box<dyn FnOnce()>) {
        self.tasks.borrow_mut().push(task);
    }
    fn navigate(&self, request: NavigationRequest) {
        self.navigations.borrow_mut().push(format!("{} {}", request.method, request.url));
    }
}

struct Env {
    _engine: Engine,
    page: Rc<PageDom>,
    main: Realm,
    hooks: Rc<Hooks>,
}

struct NoFetch;
impl lp_render::Fetcher for NoFetch {
    fn fetch(&self, url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(Err(format!("{url}: offline")));
    }
}

fn env(html: &str) -> Env {
    lp_layout::install(std::sync::Arc::new(NoFetch));
    let engine = Engine::new().expect("engine");
    register(&engine);
    let window_class = engine.register::<TestWindow>();
    let dom: DomRef = Rc::new(RefCell::new(Dom::new()));
    let doc = parse_document(&dom, "http://localhost:5173/app", html);
    dom.borrow_mut().set_document_active(doc, true).unwrap();
    let page = PageDom::new(dom, doc);
    let mut vp = page.viewport();
    vp.width = 1440;
    vp.height = 900;
    page.set_viewport(vp);
    page.layout.borrow_mut().style_mut().discover_style_elements(&page.dom.borrow());
    let hooks = Rc::new(Hooks::default());
    page.set_hooks(Some(hooks.clone()));
    let realm = engine
        .create_realm(RealmOptions { name: "main".into(), global_class: Some(window_class), origin: None })
        .expect("realm");
    let key = page.window_target().unwrap_or_else(|| page.create_window_target(page.document()));
    realm
        .set_global_data::<TestWindow>(TestWindowData {
            page: page.clone(),
            target: EventTargetData::window(&page, key),
        })
        .unwrap();
    {
        let s = realm.scope().unwrap();
        install(&s, page.clone()).unwrap();
    }
    Env { _engine: engine, page, main: realm, hooks }
}

impl Env {
    fn eval(&self, src: &str) -> String {
        let s = self.main.scope().unwrap();
        match s.eval(src, Some("test.js"), 1) {
            Ok(v) => v.to_string().unwrap(),
            Err(e) => format!("THROW {}", e.details(&s).text()),
        }
    }
    fn navigations(&self) -> Vec<String> {
        std::mem::take(&mut *self.hooks.navigations.borrow_mut())
    }
    /// Runs the posted tasks (and those they post) until none is left.
    fn run_tasks(&self) {
        loop {
            let tasks = std::mem::take(&mut *self.hooks.tasks.borrow_mut());
            if tasks.is_empty() {
                return;
            }
            for task in tasks {
                task();
                self.page.after_task();
            }
        }
    }
    fn key(&self, key: &str) {
        let k = input::KeyInput { key: key.into(), code: key.into(), ..Default::default() };
        input::dispatch_key(&self.page, input::KeyKind::RawKeyDown, &k);
        input::dispatch_key(&self.page, input::KeyKind::KeyUp, &k);
    }
    fn click_at(&self, x: f64, y: f64) {
        let m = input::MouseInput {
            x,
            y,
            button: input::MouseButton::Left,
            buttons: 1,
            click_count: 1,
            ..Default::default()
        };
        input::dispatch_mouse(&self.page, input::MouseKind::Pressed, &m).unwrap();
        let up = input::MouseInput { buttons: 0, ..m };
        input::dispatch_mouse(&self.page, input::MouseKind::Released, &up).unwrap();
    }
}

#[test]
fn toggle_and_command_event_interfaces() {
    let e = env("<body><button id=b></button></body>");
    assert_eq!(
        e.eval(
            "(() => {
                const t = new ToggleEvent('toggle', {oldState: 'closed', newState: 'open', source: b});
                const d = new ToggleEvent('beforetoggle');
                const c = new CommandEvent('command', {command: '--Go', source: b});
                const empty = new CommandEvent('command');
                let threw = '';
                try { new CommandEvent('command', {source: {}}); } catch (err) { threw = err.constructor.name; }
                return [t.oldState, t.newState, t.source === b, d.oldState === '', d.source,
                    c.command, c.source === b, empty.command === '', empty.source, threw,
                    t instanceof Event, 'onbeforetoggle' in b, 'oncommand' in b].join();
            })()"
        ),
        "closed,open,true,true,,--Go,true,true,,TypeError,true,true,true"
    );
    e.page.dispose();
}

#[test]
fn popover_events_are_sync_beforetoggle_and_coalesced_toggle() {
    let e = env("<body><div id=p popover>P</div></body>");
    assert_eq!(
        e.eval(
            "window.log = [];
            p.addEventListener('beforetoggle', ev => log.push('before ' + ev.oldState + '>' + ev.newState + ' ' + ev.cancelable));
            p.addEventListener('toggle', ev => log.push('toggle ' + ev.oldState + '>' + ev.newState + ' ' + (ev instanceof ToggleEvent)));
            p.showPopover();
            log.push('open ' + p.matches(':popover-open'));
            log.join('|')"
        ),
        "before closed>open true|open true"
    );
    e.run_tasks();
    assert_eq!(e.eval("log.splice(0).join('|')"), "before closed>open true|open true|toggle closed>open true");
    // Hidden and shown again before the task ran: one toggle, open to open.
    e.eval("p.hidePopover(); p.showPopover();");
    e.run_tasks();
    assert_eq!(
        e.eval("log.splice(0).join('|')"),
        "before open>closed false|before closed>open true|toggle open>open true"
    );
    // A canceled beforetoggle keeps it closed.
    assert_eq!(
        e.eval(
            "p.hidePopover(); p.addEventListener('beforetoggle', ev => { if (ev.newState === 'open') ev.preventDefault(); }, {once: true});
            p.showPopover(); p.matches(':popover-open')"
        ),
        "false"
    );
    e.page.dispose();
}

#[test]
fn auto_popover_stack_and_nesting() {
    let e = env("<body><div id=a popover>A<div id=a2 popover>A2</div></div><div id=b popover>B</div>
         <div id=m popover=manual>M</div><div id=h popover=hint>H</div></body>");
    let open = "[a, a2, b, m, h].map(x => x.matches(':popover-open') ? 1 : 0).join('')";
    // A nested auto popover keeps its ancestor open.
    e.eval("m.showPopover(); a.showPopover(); a2.showPopover();");
    assert_eq!(e.eval(open), "11010");
    // An unrelated one closes both; a manual popover stays.
    e.eval("b.showPopover();");
    assert_eq!(e.eval(open), "00110");
    // A hint does not close autos; an auto closes hints.
    e.eval("h.showPopover();");
    assert_eq!(e.eval(open), "00111");
    e.eval("a.showPopover();");
    assert_eq!(e.eval(open), "10010");
    // Hiding an ancestor hides what it nests.
    e.eval("a2.showPopover(); a.hidePopover();");
    assert_eq!(e.eval(open), "00010");
    e.page.dispose();
}

#[test]
fn popovertarget_buttons() {
    let e = env("<body><div id=p popover>P</div>
         <button id=t popovertarget=p>t</button>
         <button id=s popovertarget=p popovertargetaction=show>s</button>
         <button id=h popovertarget=p popovertargetaction=hide>h</button>
         <input id=i type=button popovertarget=p>
         <form><button id=f popovertarget=p>f</button></form></body>");
    let open = "p.matches(':popover-open')";
    e.eval("t.click()");
    assert_eq!(e.eval(open), "true");
    e.eval("t.click()");
    assert_eq!(e.eval(open), "false");
    e.eval("h.click()");
    assert_eq!(e.eval(open), "false");
    e.eval("s.click(); s.click()");
    assert_eq!(e.eval(open), "true");
    e.eval("h.click()");
    assert_eq!(e.eval(open), "false");
    e.eval("i.click()");
    assert_eq!(e.eval(open), "true");
    e.eval("i.click()");
    // A submit button with a form owner submits instead.
    e.eval("document.forms[0].onsubmit = () => false; f.click()");
    assert_eq!(e.eval(open), "false");
    // The invoker is the toggle events' source.
    e.eval("window.src = []; p.addEventListener('toggle', ev => src.push(ev.source && ev.source.id)); t.click();");
    e.run_tasks();
    assert_eq!(e.eval("src.join()"), "t");
    assert_eq!(
        e.eval(
            "[t.popoverTargetElement === p, s.popoverTargetAction, i.popoverTargetAction, (h.popoverTargetAction = 'bogus', h.popoverTargetAction),
              (t.popoverTargetElement = null, t.hasAttribute('popovertarget')),
              (t.popoverTargetElement = p, t.getAttribute('popovertarget') === '' && t.popoverTargetElement === p)].join()"
        ),
        "true,show,toggle,toggle,false,true"
    );
    e.page.dispose();
}

#[test]
fn command_invokers() {
    let e = env("<body><dialog id=d><button id=cl commandfor=d command=close value=done>x</button>
           <button id=rc commandfor=d command=request-close value=asked>r</button></dialog>
         <button id=sm commandfor=d command=show-modal>o</button>
         <div id=p popover>P</div>
         <button id=tp commandfor=p command=toggle-popover>tp</button>
         <button id=bad commandfor=p command=show-modal>bad</button>
         <button id=custom commandfor=p command=--Refresh>c</button>
         <form id=form><button id=inform commandfor=p command=toggle-popover>f</button></form></body>");
    assert_eq!(
        e.eval(
            "window.cmds = [];
            for (const t of [d, p]) t.addEventListener('command', ev => cmds.push(ev.command + ':' + ev.source.id + ':' + (ev instanceof CommandEvent)));
            sm.click();
            const modal = d.matches(':modal');
            cl.click();
            [modal, d.open, d.returnValue, cmds.join()].join()"
        ),
        "true,false,done,show-modal:sm:true,close:cl:true"
    );
    assert_eq!(
        e.eval(
            "cmds.length = 0;
            sm.click();
            d.addEventListener('cancel', ev => ev.preventDefault(), {once: true});
            rc.click();
            const kept = d.open;
            rc.click();
            [kept, d.open, d.returnValue].join()"
        ),
        "true,false,asked"
    );
    assert_eq!(
        e.eval(
            "cmds.length = 0;
            tp.click(); const shown = p.matches(':popover-open'); tp.click();
            bad.click(); custom.click();
            p.addEventListener('command', ev => ev.preventDefault(), {once: true}); tp.click();
            let submitted = false; form.onsubmit = () => { submitted = true; return false; }; inform.click(); inform.click();
            [shown, p.matches(':popover-open'), cmds.join(' '), submitted, inform.type, custom.command, bad.command,
             (bad.command = 'Nope', bad.command), tp.commandForElement === p].join()"
        ),
        "true,false,toggle-popover:tp:true toggle-popover:tp:true --Refresh:custom:true toggle-popover:tp:true toggle-popover:inform:true toggle-popover:inform:true,false,button,--Refresh,show-modal,,true"
    );
    e.page.dispose();
}

#[test]
fn details_toggle_events_and_exclusive_accordion() {
    let e = env(
        "<body><details id=a name=g><summary id=sa>a</summary></details><details id=b name=g open><summary>b</summary></details>
         <details id=c><summary>c</summary></details></body>",
    );
    // Parsed with `open`: a toggle event too.
    e.eval(
        "document.body.insertAdjacentHTML('beforeend', '<details id=parsed open><summary>s</summary></details>');
        parsed.addEventListener('toggle', ev => window.parsedToggle = ev.oldState + '>' + ev.newState);
        window.log = [];
        for (const d of [a, b, c]) d.addEventListener('toggle', ev => log.push(d.id + ' ' + ev.oldState + '>' + ev.newState + ' ' + (ev instanceof ToggleEvent) + ' ' + ev.bubbles + ' ' + ev.cancelable));",
    );
    e.run_tasks();
    assert_eq!(e.eval("window.parsedToggle"), "closed>open");
    e.eval("log.length = 0");
    // Opening one of a group closes the open one; the events come later.
    assert_eq!(e.eval("a.open = true; [a.open, b.open, log.length].join()"), "true,false,0");
    e.run_tasks();
    assert_eq!(
        e.eval("log.splice(0).sort().join('|')"),
        "a closed>open true false false|b open>closed true false false"
    );
    // Coalesced: open then closed before the task is one closed>closed event.
    e.eval("c.open = true; c.removeAttribute('open');");
    e.run_tasks();
    assert_eq!(e.eval("log.splice(0).join('|')"), "c closed>closed true false false");
    // Setting the same state fires nothing; a summary click toggles.
    e.eval("c.open = false; document.getElementById('sa').click();");
    e.run_tasks();
    assert_eq!(e.eval("[a.open, log.splice(0).join('|')].join()"), "false,a open>closed true false false");
    // Inserting an open details into a group with an open one closes it.
    assert_eq!(
        e.eval(
            "a.open = true; const n = document.createElement('details'); n.name = 'g'; n.open = true;
             document.body.append(n); [a.open, n.open].join()"
        ),
        "true,false"
    );
    e.page.dispose();
}

#[test]
fn dialog_close_requests_and_closed_by() {
    let e = env("<body><button id=out>out</button><dialog id=d><p>hi</p></dialog></body>");
    assert_eq!(
        e.eval(
            "window.log = [];
            d.addEventListener('cancel', ev => log.push('cancel ' + ev.cancelable));
            d.addEventListener('close', () => log.push('close'));
            out.focus();
            d.showModal();
            [d.closedBy, document.activeElement === d].join()"
        ),
        "closerequest,true"
    );
    e.key("Escape");
    assert_eq!(e.eval("[d.open, d.matches(':modal'), log.join()].join()"), "false,false,cancel true");
    e.run_tasks();
    // Focus goes back to where it was.
    assert_eq!(e.eval("[log.join(), document.activeElement === out].join()"), "cancel true,close,true");
    // A canceled cancel keeps it open; closedby=none ignores Escape.
    e.eval("log.length = 0; d.showModal(); d.addEventListener('cancel', ev => ev.preventDefault(), {once: true});");
    e.key("Escape");
    assert_eq!(e.eval("d.open"), "true");
    e.eval("d.setAttribute('closedby', 'none')");
    e.key("Escape");
    assert_eq!(e.eval("[d.open, d.closedBy].join()"), "true,none");
    // requestClose fires cancel whatever closedby says.
    e.eval("d.requestClose('rv')");
    assert_eq!(e.eval("[d.open, d.returnValue, (d.removeAttribute('closedby'), d.closedBy)].join()"), "false,rv,none");
    // A non-modal dialog listens to Escape only with closedby.
    e.eval("d.show()");
    e.key("Escape");
    assert_eq!(e.eval("d.open"), "true");
    e.eval("d.close(); d.closedBy = 'ANY'; d.show()");
    e.key("Escape");
    assert_eq!(e.eval("[d.open, d.closedBy].join()"), "false,any");
    e.page.dispose();
}

#[test]
fn dialog_focusing_steps_and_method_dialog() {
    let e = env(
        "<body><dialog id=d><button id=first>1</button><input id=auto autofocus></dialog>
         <dialog id=plain><button disabled>x</button><button id=only>y</button></dialog>
         <dialog id=empty><p>nothing</p></dialog>
         <dialog id=fd><form method=dialog><button id=ok value=yes>OK</button><button id=no>No</button></form></dialog></body>",
    );
    assert_eq!(e.eval("d.showModal(); document.activeElement.id"), "auto");
    assert_eq!(e.eval("d.close(); plain.show(); document.activeElement.id"), "only");
    assert_eq!(e.eval("plain.close(); empty.showModal(); document.activeElement === empty"), "true");
    assert_eq!(
        e.eval(
            "empty.close(); fd.showModal(); ok.click();
            const first = [fd.open, fd.returnValue];
            fd.showModal(); no.click();
            [...first, fd.open, fd.returnValue].join()"
        ),
        "false,yes,false,yes"
    );
    assert_eq!(e.navigations(), Vec::<String>::new());
    e.page.dispose();
}

#[test]
fn modal_dialog_makes_the_rest_inert_and_is_hit_first() {
    let e = env(
        "<body style='margin:0'><button id=out style='position:absolute;left:0;top:0;width:100px;height:40px;z-index:10'>out</button>
         <dialog id=d style='margin:0;left:200px;top:200px;width:100px;height:100px;padding:0;border:0'><button id=in>in</button></dialog>
         <div id=wrap style='position:relative;z-index:100'><div id=p popover style='margin:0;left:0;top:0;width:50px;height:50px;padding:0;border:0'>p</div></div>
         <div id=cover style='position:absolute;left:0;top:0;width:60px;height:60px;z-index:1000'></div></body>",
    );
    // A showing popover is above a z-index:1000 sibling.
    assert_eq!(e.eval("p.showPopover(); document.elementFromPoint(10, 10).id"), "p");
    e.eval("p.hidePopover()");
    assert_eq!(e.eval("document.elementFromPoint(10, 10).id"), "cover");
    e.eval("cover.remove()");
    assert_eq!(e.eval("document.elementFromPoint(10, 10).id"), "out");
    assert_eq!(
        e.eval(
            "d.showModal();
            out.focus();
            [document.activeElement.id, document.elementFromPoint(10, 10) === d, document.elementFromPoint(210, 210).id].join()"
        ),
        "in,true,in"
    );
    // Tab stays inside; a click outside does not reach the button.
    e.eval("window.clicks = 0; out.onclick = () => clicks++;");
    e.key("Tab");
    assert_eq!(e.eval("document.activeElement.id"), "in");
    e.click_at(10.0, 10.0);
    assert_eq!(e.eval("[clicks, d.open].join()"), "0,true");
    // closedby=any closes on a click outside, not inside.
    e.eval("d.closedBy = 'any'");
    e.click_at(250.0, 280.0);
    assert_eq!(e.eval("d.open"), "true");
    e.click_at(10.0, 10.0);
    assert_eq!(e.eval("[d.open, clicks].join()"), "false,0");
    e.click_at(10.0, 10.0);
    assert_eq!(e.eval("clicks"), "1");
    e.page.dispose();
}

#[test]
fn popover_light_dismiss_and_escape() {
    let e = env(
        "<body style='margin:0'><button id=out style='position:absolute;left:0;top:300px;width:100px;height:40px'>out</button>
         <button id=inv popovertarget=p style='position:absolute;left:200px;top:300px;width:100px;height:40px'>inv</button>
         <div id=p popover style='margin:0;left:0;top:0;width:100px;height:100px;padding:0;border:0'><span id=inner>p</span></div>
         <div id=q popover style='margin:0;left:200px;top:0;width:100px;height:100px;padding:0;border:0'>q</div></body>",
    );
    let open = "[p, q].map(x => x.matches(':popover-open') ? 1 : 0).join('')";
    // q nests in p through its invoker.
    e.eval("p.showPopover(); q.showPopover({source: inner});");
    assert_eq!(e.eval(open), "11");
    assert_eq!(e.eval("document.elementFromPoint(250, 50).id"), "q");
    // A click in the outer popover closes the nested one only.
    e.click_at(50.0, 50.0);
    assert_eq!(e.eval(open), "10");
    // A click on its invoker toggles it (light dismiss leaves it alone).
    e.click_at(250.0, 320.0);
    assert_eq!(e.eval(open), "00");
    e.click_at(250.0, 320.0);
    assert_eq!(e.eval(open), "10");
    e.click_at(50.0, 320.0);
    assert_eq!(e.eval(open), "00");
    // Escape hides the topmost one.
    e.eval("p.showPopover(); q.showPopover({source: inner});");
    e.key("Escape");
    assert_eq!(e.eval(open), "10");
    e.key("Escape");
    assert_eq!(e.eval(open), "00");
    e.page.dispose();
}

#[test]
fn removing_open_from_a_modal_dialog_unblocks_the_document() {
    let e = env("<body><button id=out>out</button><dialog id=d><button id=in>in</button></dialog></body>");
    assert_eq!(
        e.eval(
            "d.showModal(); out.focus(); const blocked = document.activeElement.id;
            d.removeAttribute('open'); out.focus();
            [blocked, d.matches(':modal'), document.activeElement.id].join()"
        ),
        "in,false,out"
    );
    e.page.dispose();
}
