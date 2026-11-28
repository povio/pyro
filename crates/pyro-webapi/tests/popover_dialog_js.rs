//! Popovers, dialogs, invokers and details as a page sees them:
//! `beforetoggle`/`toggle` (`ToggleEvent`), the auto popover stack, light
//! dismiss and Escape, `popovertarget` and `command`/`commandfor`
//! (`CommandEvent`), the dialog focusing steps, `requestClose`/`closedBy`,
//! `method=dialog`, modal inertness and the top layer's hit testing
//! (html/semantics/popovers, the-dialog-element, the-details-element,
//! the-button-element/command-and-commandfor).

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
fn command_and_toggle_source_follow_the_observing_listener() {
    let e = env("<body><div id=host></div></body>");
    assert_eq!(
        e.eval(
            "(() => {
                const root = host.attachShadow({mode:'closed'});
                const button = root.appendChild(document.createElement('button'));
                const target = root.appendChild(document.createElement('span'));
                const peer = document.body.appendChild(document.createElement('div'));
                let index = 0;
                for (const Interface of [CommandEvent, ToggleEvent]) {
                    for (const source of [target, button, host, null, peer]) {
                        const type = 'case' + index++;
                        const crosses = source === null || source === peer;
                        const ev = new Interface(type, {source, bubbles:true, composed:true});
                        const outside = source === button || source === target ? host : source;
                        if (new Interface(type, {source}).composed || ev.source !== outside) return false;
                        const seen = [];
                        for (const capture of [true, false]) {
                            host.addEventListener(type, e => seen.push(e.source === outside), capture);
                            root.addEventListener(type, e => seen.push(e.source === source), capture);
                            target.addEventListener(type, e => {
                                const path = e.composedPath();
                                seen.push(e.source === source && path.includes(host) === crosses &&
                                    path[0] === target && path[1] === root);
                            }, capture);
                        }
                        target.dispatchEvent(ev);
                        if (seen.length !== (crosses ? 6 : 4) || !seen.every(Boolean) || ev.source !== outside)
                            return false;
                    }
                }
                return true;
            })()"
        ),
        "true"
    );
    e.page.dispose();
}

#[test]
fn command_path_ends_below_the_host_that_hides_both_source_and_target() {
    let e = env("<body><div id=outer></div></body>");
    assert_eq!(
        e.eval(
            "(() => {
                const root = outer.attachShadow({mode:'open'});
                const inner = root.appendChild(document.createElement('div'));
                const button = inner.attachShadow({mode:'open'}).appendChild(document.createElement('button'));
                const log = [];
                root.addEventListener('custom', e => log.push(e.source === inner), true);
                inner.addEventListener('custom', e => {
                    const path = e.composedPath();
                    log.push(path.length === 2 && path[0] === inner && path[1] === root);
                });
                outer.addEventListener('custom', () => log.push(false), true);
                document.addEventListener('custom', () => log.push(false), true);
                inner.dispatchEvent(new CommandEvent('custom', {source:button, composed:true}));
                return log.join();
            })()"
        ),
        "true,true"
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
fn attribute_close_runs_nested_changes_before_returning_to_script() {
    let e = env("<body><div id=a popover><div id=p popover></div></div></body>");
    e.eval("a.showPopover(); p.showPopover()");
    e.run_tasks();
    assert_eq!(
        e.eval(
            "window.events = [];
             p.addEventListener('toggle', ev => events.push(ev.type));
             p.addEventListener('beforetoggle', ev => {
                 events.push(ev.newState);
                 p.popover = 'manual';
                 events.push(p.matches(':popover-open'));
             }, {once:true});
             p.getAttributeNode('popover').value = 'hint';
             [a.matches(':popover-open'), p.matches(':popover-open'), p.popover, ...events].join()"
        ),
        "true,false,manual,closed,false"
    );
    e.run_tasks();
    assert_eq!(e.eval("events.join()"), "closed,false");
    e.eval("p.popover = 'auto'; p.showPopover()");
    assert_eq!(
        e.eval(
            "const child = p; child.addEventListener('beforetoggle', () => child.remove(), {once:true});
             child.removeAttribute('popover');
             [child.isConnected, a.matches(':popover-open')].join()"
        ),
        "false,true"
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
fn command_activation_precedes_popover_applicability_and_preserves_dialog_invoker() {
    let e = env("<body><button id=b type=button commandfor=p command=show-popover></button>
        <div id=p></div><dialog id=d><button id=r type=button commandfor=d command=request-close></button></dialog></body>");
    assert_eq!(e.eval(
        "const b=document.getElementById('b'), p=document.getElementById('p'), d=document.getElementById('d'),
               r=document.getElementById('r'), seen=[];
         p.addEventListener('command', ev => {
           seen.push(ev.isTrusted, ev.composed, ev.cancelable, ev.bubbles, ev.source===b);
           p.setAttribute('popover','manual');
         }, {once:true});
         b.click();
         seen.push(p.matches(':popover-open'));
         p.hidePopover();
         b.commandForElement=d; b.command='show-modal';
         d.addEventListener('beforetoggle', ev => seen.push(ev.newState, ev.source===b, ev.source===r));
         b.click(); r.click();
         seen.join()"
    ), "true,true,true,false,true,true,open,true,false,closed,false,true");
    e.page.dispose();
}

#[test]
fn command_target_reflection_observes_content_attribute_mutations() {
    let e = env("<body><button id=inv command=--preview></button><div id=a></div><div id=b></div></body>");
    assert_eq!(e.eval(
        "const inv=document.getElementById('inv'), a=document.getElementById('a'), b=document.getElementById('b');
         const result=[], clicks=[];
         for (const target of [a,b]) target.addEventListener('command', () => clicks.push(target.id));
         function check() {
           const count=clicks.length;
           inv.click();
           result.push((inv.commandForElement?.id ?? '-') + '/' + (clicks[count] ?? '-'));
         }
         inv.commandForElement=a; check();
         inv.setAttribute('commandfor','b'); check();
         inv.setAttribute('commandfor',''); check();
         inv.commandForElement=a; inv.getAttributeNode('commandfor').value=''; check();
         inv.commandForElement=a; inv.setAttributeNS('urn:other','x:commandfor','b'); check();
         inv.removeAttributeNS('urn:other','commandfor'); check();
         inv.removeAttribute('commandfor'); check();
         inv.setAttribute('commandfor',''); check();
         result.join()"
    ), "a/a,b/b,-/-,-/-,a/a,a/a,-/-,-/-");
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
         <form id=form><button id=inform commandfor=p command=toggle-popover>f</button>
           <button id=btn type=button commandfor=p command=toggle-popover>b</button>
           <button id=sub type=submit commandfor=p command=toggle-popover>s</button>
           <button id=rst type=reset commandfor=p command=toggle-popover>r</button>
           <button id=cmdonly command=toggle-popover>c</button></form>
         <button id=out type=reset commandfor=p command=toggle-popover>o</button></body>");
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
        "true,false,toggle-popover:tp:true toggle-popover:tp:true --Refresh:custom:true toggle-popover:tp:true,false,button,--Refresh,show-modal,,true"
    );
    // With a form owner, only `type=button` runs its command: a submit or
    // reset button does only that, the Auto state nothing
    // (button-type-behavior.html); outside a form every type does.
    assert_eq!(
        e.eval(
            "cmds.length = 0; let submits = 0, resets = 0;
            form.onsubmit = () => { submits++; return false; }; form.onreset = () => { resets++; };
            sub.click(); const afterSubmit = p.matches(':popover-open');
            rst.click(); const afterReset = p.matches(':popover-open');
            cmdonly.click();
            btn.click(); const afterButton = p.matches(':popover-open'); btn.click();
            out.click(); const afterOut = p.matches(':popover-open'); out.click();
            [submits, resets, afterSubmit, afterReset, afterButton, afterOut, cmdonly.type, sub.type, cmds.join(' ')].join()"
        ),
        "1,1,false,false,true,true,button,submit,toggle-popover:btn:true toggle-popover:btn:true toggle-popover:out:true toggle-popover:out:true"
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
    // (A close request's `cancel` is cancelable only while the window has history-action activation,
    // which Escape itself is no input for: Chrome fires `cancel false` for a dialog a script opened
    // and nothing the user did since. A key press before each Escape gives the activation.)
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
    e.key("Shift");
    e.key("Escape");
    assert_eq!(e.eval("[d.open, d.matches(':modal'), log.join()].join()"), "false,false,cancel true");
    e.run_tasks();
    // Focus goes back to where it was.
    assert_eq!(e.eval("[log.join(), document.activeElement === out].join()"), "cancel true,close,true");
    // A canceled cancel keeps it open; closedby=none ignores Escape.
    e.eval("log.length = 0; d.showModal(); d.addEventListener('cancel', ev => ev.preventDefault(), {once: true});");
    e.key("Shift");
    e.key("Escape");
    assert_eq!(e.eval("d.open"), "true");
    e.eval("d.setAttribute('closedby', 'none')");
    e.key("Shift");
    e.key("Escape");
    assert_eq!(e.eval("[d.open, d.closedBy].join()"), "true,none");
    // requestClose fires cancel whatever closedby says.
    e.eval("d.requestClose('rv')");
    assert_eq!(e.eval("[d.open, d.returnValue, (d.removeAttribute('closedby'), d.closedBy)].join()"), "false,rv,none");
    // A non-modal dialog listens to Escape only with closedby.
    e.eval("d.show()");
    e.key("Shift");
    e.key("Escape");
    assert_eq!(e.eval("d.open"), "true");
    e.eval("d.close(); d.closedBy = 'ANY'; d.show()");
    e.key("Shift");
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

#[test]
fn dialog_show_hides_popovers_that_do_not_contain_it() {
    let e = env("<body><div id=p popover>p<dialog id=inner>i</dialog></div><div id=q popover>q</div>
         <dialog id=outer>o</dialog></body>");
    assert_eq!(
        e.eval(
            "p.showPopover(); inner.show();
            const kept = p.matches(':popover-open');
            outer.show();
            [kept, p.matches(':popover-open'), inner.open, outer.open].join()"
        ),
        "true,false,true,true"
    );
    e.page.dispose();
}

#[test]
fn details_summary_keyboard_activation_and_cancellation() {
    let e = env("<body><details id=d><summary id=s>Toggle</summary><summary id=other tabindex=0>Other</summary></details><summary id=orphan tabindex=0>Orphan</summary></body>");
    e.eval("window.clicks = []; s.addEventListener('click', e => clicks.push([e.isTrusted, e.detail, e.pointerId].join(':'))); s.focus()");
    e.key("Enter");
    assert_eq!(e.eval("[d.open, clicks.join()].join()"), "true,true:0:-1");
    let space = input::KeyInput { key: " ".into(), code: "Space".into(), ..Default::default() };
    input::dispatch_key(&e.page, input::KeyKind::RawKeyDown, &space);
    assert_eq!(e.eval("d.open"), "true");
    input::dispatch_key(&e.page, input::KeyKind::KeyUp, &space);
    assert_eq!(e.eval("d.open"), "false");
    for event in ["keydown", "click"] {
        e.eval(&format!("s.addEventListener('{event}', e => e.preventDefault(), {{once:true}})"));
        e.key("Enter");
        assert_eq!(e.eval("d.open"), "false");
    }
    e.eval("s.addEventListener('keyup', e => e.preventDefault(), {once:true})");
    e.key(" ");
    assert_eq!(e.eval("d.open"), "false");
    for id in ["other", "orphan"] {
        e.eval(&format!("document.getElementById('{id}').focus()"));
        e.key("Enter");
        e.key(" ");
        assert_eq!(e.eval("d.open"), "false");
    }
    e.page.dispose();
}

/// HTML "hide all popovers until" terminates whatever the page's script does: a focus handler
/// that opens another child popover each time focus comes back to it does not keep it hiding
/// forever. The popovers above the endpoint close without moving focus; focus goes back once,
/// to where it was before the lowest of them opened (the handler may then open one more,
/// which the next step closes).
#[test]
fn hiding_popovers_until_an_endpoint_ends_whatever_focus_handlers_do() {
    let e = env(
        "<div popover id=parent><button id=btn>b</button>\
         <div popover id=a><input id=ia autofocus></div>\
         <div popover id=b><input id=ib autofocus></div></div>",
    );
    assert_eq!(
        e.eval(
            "window.opened = 0;\
             parent.showPopover(); btn.focus(); a.showPopover();\
             btn.addEventListener('focus', () => { if (++opened < 200) (opened % 2 ? b : a).showPopover(); });\
             document.activeElement.id"
        ),
        "ia"
    );
    e.eval("parent.hidePopover()");
    // Focus goes back once when its children close (the handler opens `b`), and once more
    // when the popover the handler opened is closed by the check after `beforetoggle` (it
    // opens `a`): not 200 times.
    assert_eq!(e.eval("opened"), "2", "focus was restored once per hide-all-until");
    assert_eq!(e.eval("parent.matches(':popover-open')"), "false");
}
