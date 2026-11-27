//! The streaming parsing methods (`streamHTML` and friends and their `Unsafe` twins): a
//! `WritableStream` feeding the incremental fragment parser. The safe ones are not Trusted Types
//! sinks and always filter with the default sanitizer and the built-in safe baseline (the
//! `SetHTMLOptions` conversion: `runScripts` is neither read nor honoured); on a `script` element
//! all of them throw `NotSupportedError` (the non-streaming safe methods do nothing there), before
//! the options are looked at.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use pyro_dom::parse::{ParseStep, StreamingFragmentParser};
use pyro_dom::NodeType;
use pyro_js::{Args, ClassDef, Deferred, JsClass, JsError, JsResult, Scope, Value, WeakRealm};

use crate::node::NodeData;
use crate::page::{PageDom, StreamScriptJob};
use crate::util::{dom_string, DomResultExt};

#[derive(Clone, Copy, Eq, PartialEq)]
enum Position {
    ReplaceChildren,
    Append,
    Prepend,
    Before,
    After,
    Replace,
}

pub(crate) fn parents<C: JsClass<Data = NodeData>>(def: &mut ClassDef<C>) {
    def.method("streamHTMLUnsafe", 0, |s, n, a| open(s, n, a, Position::ReplaceChildren, "streamHTMLUnsafe", false));
    def.method("streamAppendHTMLUnsafe", 0, |s, n, a| open(s, n, a, Position::Append, "streamAppendHTMLUnsafe", false));
    def.method("streamPrependHTMLUnsafe", 0, |s, n, a| open(s, n, a, Position::Prepend, "streamPrependHTMLUnsafe", false));
    def.method("streamHTML", 0, |s, n, a| open(s, n, a, Position::ReplaceChildren, "streamHTML", true));
    def.method("streamAppendHTML", 0, |s, n, a| open(s, n, a, Position::Append, "streamAppendHTML", true));
    def.method("streamPrependHTML", 0, |s, n, a| open(s, n, a, Position::Prepend, "streamPrependHTML", true));
}

pub(crate) fn siblings<C: JsClass<Data = NodeData>>(def: &mut ClassDef<C>) {
    def.method("streamBeforeHTMLUnsafe", 0, |s, n, a| open(s, n, a, Position::Before, "streamBeforeHTMLUnsafe", false));
    def.method("streamAfterHTMLUnsafe", 0, |s, n, a| open(s, n, a, Position::After, "streamAfterHTMLUnsafe", false));
    def.method("streamReplaceWithHTMLUnsafe", 0, |s, n, a| {
        open(s, n, a, Position::Replace, "streamReplaceWithHTMLUnsafe", false)
    });
    def.method("streamBeforeHTML", 0, |s, n, a| open(s, n, a, Position::Before, "streamBeforeHTML", true));
    def.method("streamAfterHTML", 0, |s, n, a| open(s, n, a, Position::After, "streamAfterHTML", true));
    def.method("streamReplaceWithHTML", 0, |s, n, a| open(s, n, a, Position::Replace, "streamReplaceWithHTML", true));
}

/// Whether the parsing context of a stream into / next to `id` is an HTML or SVG `script` element
/// (an unresolvable target is not: that is `open`'s HierarchyRequestError, after the options).
fn targets_script(d: &pyro_dom::Dom, id: pyro_dom::NodeId, sibling: bool) -> bool {
    let target = if sibling {
        match d.parent(id) {
            Ok(Some(parent)) => parent,
            _ => return false,
        }
    } else {
        d.template_contents(id).ok().flatten().unwrap_or(id)
    };
    let context = if d.node_type(target).ok() == Some(NodeType::Element) {
        target
    } else {
        match d.fragment_host(target) {
            Ok(Some(host)) => host,
            _ => return false,
        }
    };
    crate::positional_html::is_script_element(d, context)
}

struct State {
    page: Rc<PageDom>,
    realm: WeakRealm,
    parser: RefCell<Option<StreamingFragmentParser>>,
    alive: Rc<Cell<bool>>,
    closed: Cell<bool>,
    driving: Cell<bool>,
    abort_requested: Cell<bool>,
    queued: RefCell<std::collections::VecDeque<String>>,
    running_deferred: Cell<bool>,
    deferred: RefCell<std::collections::VecDeque<Rc<RefCell<Option<StreamScriptJob>>>>>,

    close: RefCell<Option<Deferred>>,
}

impl State {
    fn stop(&self) {
        self.alive.set(false);
        self.parser.borrow_mut().take();
        self.deferred.borrow_mut().clear();
        self.queued.borrow_mut().clear();
    }

    fn close_parser(&self) -> JsResult<bool> {
        let result = {
            let mut parser = self.parser.borrow_mut();
            parser.as_mut().ok_or_else(|| JsError::dom("AbortError", "Parser ended"))?.close().js()
        };
        if result.is_err() {
            self.stop();
        }
        result
    }

    fn finish(self: &Rc<Self>) {
        self.parser.borrow_mut().take();
        self.closed.set(true);
        self.run_deferred();
    }

    fn run_deferred(self: &Rc<Self>) {
        if !self.closed.get() || self.running_deferred.get() {
            return;
        }
        let next = self.deferred.borrow().front().and_then(|slot| slot.borrow_mut().take());
        if let Some(run) = next {
            self.deferred.borrow_mut().pop_front();
            self.running_deferred.set(true);
            let state = self.clone();
            run(Box::new(move || {
                state.running_deferred.set(false);
                state.run_deferred();
            }));
        }
    }

    fn drive(self: &Rc<Self>, s: &Scope<'_>, controller: Value<'_>, first: Option<String>) -> JsResult<()> {
        if self.driving.replace(true) {
            if let Some(chunk) = first {
                self.queued.borrow_mut().push_back(chunk);
            }
            return Ok(());
        }
        let result = (|| {
            let mut chunk = first;
            loop {
                let step = {
                    let mut parser = self.parser.borrow_mut();
                    let parser = parser.as_mut().ok_or_else(|| JsError::dom("AbortError", "Parser ended"))?;
                    match chunk {
                        Some(text) => parser.write(&text),
                        None => parser.resume(),
                    }
                    .js()?
                };
                self.advance(s, controller, step)?;
                if self.abort_requested.get() {
                    self.stop();
                    break;
                }
                chunk = self.queued.borrow_mut().pop_front();
                if chunk.is_none() {
                    break;
                }
            }
            if self.alive.get() && self.close.borrow().is_some() {
                if self.parser.borrow_mut().as_mut().is_some_and(|p| p.is_finished()) {
                    if let Some(d) = self.close.borrow_mut().take() {
                        d.resolve(s, s.undefined())?;
                    }
                    self.finish();
                } else {
                    let finished = self.close_parser()?;
                    if finished {
                        if let Some(d) = self.close.borrow_mut().take() {
                            d.resolve(s, s.undefined())?;
                        }
                        self.finish();
                    }
                }
            }
            Ok(())
        })();
        self.driving.set(false);
        result
    }
    fn advance(self: &Rc<Self>, s: &Scope<'_>, controller: Value<'_>, mut step: ParseStep) -> JsResult<()> {
        while let ParseStep::Script(node) = step {
            let doc = self.page.dom.borrow().node_document(node).js()?;
            let Some(script_page) = crate::custom::document_page(&self.page, doc) else {
                step = self.parser.borrow_mut().as_mut().unwrap().resume().js()?;
                continue;
            };
            let info = self.page.dom.borrow().script_info(node).js()?;
            let module = info.kind == pyro_dom::script::ScriptKind::Module;
            if module || (info.kind == pyro_dom::script::ScriptKind::Classic && !info.nomodule && info.src.is_some()) {
                let blocking = !module && !info.async_ && !info.defer;
                let deferred = (!info.async_ && (module || info.defer)).then(|| {
                    let slot = Rc::new(RefCell::new(None));
                    self.deferred.borrow_mut().push_back(Rc::clone(&slot));
                    slot
                });
                self.page.dom.borrow_mut().mark_script_started(node).js()?;
                let held = crate::collections::NodeHolds::new(&self.page, [node])?;
                let controller = pyro_js::Persistent::new(s, controller);
                let state = self.clone();
                let live = self.alive.clone();
                let hooks = script_page.hooks().ok_or_else(|| JsError::dom("NotSupportedError", "No script loader"))?;
                hooks.streamed_script(
                    node,
                    Rc::new(move || live.get()),
                    Box::new(move |run| {
                        if let Some(slot) = deferred {
                            if state.alive.get() {
                                *slot.borrow_mut() = Some(run);
                                state.run_deferred();
                            }
                            return;
                        }
                        run(Box::new(move || {
                            let _held = held;
                            if !blocking || !state.alive.get() {
                                return;
                            }
                            let Some(realm) = state.realm.upgrade() else {
                                state.stop();
                                return;
                            };
                            let Ok(s) = realm.scope() else {
                                state.stop();
                                return;
                            };
                            let result = state.drive(&s, controller.get(&s), None);
                            if let Err(err) = result {
                                state.stop();
                                let error = err.to_value(&s);
                                let _ = crate::streams::module(&s, &state.page).and_then(|m| {
                                    m.get_atom("errorWritableController")?
                                        .as_object()
                                        .unwrap()
                                        .call(None, &[controller.get(&s), error])
                                });
                                if let Some(d) = state.close.borrow_mut().take() {
                                    let _ = d.reject(&s, error);
                                }
                            }
                        }));
                    }),
                );
                if blocking {
                    return Ok(());
                }
            } else if info.kind == pyro_dom::script::ScriptKind::Classic && !info.nomodule {
                if let Some(hooks) = script_page.hooks() {
                    hooks.script_connected(node);
                }
            }
            let next = self.parser.borrow_mut().as_mut().unwrap().resume().js()?;
            step = next;
        }
        Ok(())
    }
}

fn open<'s>(s: &Scope<'s>, n: &NodeData, a: Args<'s>, position: Position, name: &str, safe: bool) -> JsResult<Value<'s>> {
    let page = n.page();
    let sibling = matches!(position, Position::Before | Position::After | Position::Replace);
    if targets_script(&page.dom.borrow(), n.id, sibling) {
        return Err(JsError::dom("NotSupportedError", "Cannot stream HTML into a script element"));
    }
    let receiver = if sibling {
        "Node"
    } else if page.dom.borrow().is_shadow_root(n.id).js()? {
        "ShadowRoot"
    } else {
        "Element"
    };
    let sink_name = format!("{receiver} {name}");
    let doc = page.dom.borrow().node_document(n.id).js()?;
    let owner = crate::custom::document_page(page, doc);
    let policy_page = owner.as_ref().unwrap_or(page);
    let options = a.get(0);
    let options = if safe {
        crate::sanitizer_options::ParserOptions::convert_with(s, options, crate::sanitizer_options::ParseMode::Safe)?
    } else if !Rc::ptr_eq(policy_page, page) {
        let realm = policy_page
            .main_world_realm()
            .ok_or_else(|| JsError::Type("The target document realm is unavailable".into()))?;
        let owner_scope = realm.scope()?;
        crate::trusted_types::parser_options(&owner_scope, policy_page, options, &sink_name, true)?.0
    } else {
        if owner.is_none() && crate::trusted_types::requires_for_script_in(page, doc) {
            return Err(JsError::Type("The target document parser policy is unavailable".into()));
        }
        crate::trusted_types::parser_options(s, policy_page, options, &sink_name, true)?.0
    };
    let bad_target = || JsError::dom("HierarchyRequestError", "Invalid HTML stream target");
    let (context, target, before) = {
        let d = page.dom.borrow();
        let target = if sibling {
            let p = d.parent(n.id).js()?.ok_or_else(bad_target)?;
            if d.template_contents(p).js()?.is_some() {
                return Err(bad_target());
            }
            p
        } else {
            d.template_contents(n.id).js()?.unwrap_or(n.id)
        };
        let context = if d.node_type(target).js()? == NodeType::Element {
            target
        } else {
            d.fragment_host(target).js()?.ok_or_else(bad_target)?
        };
        let el = d.element(context).js()?;
        if position != Position::ReplaceChildren && el.is_html() && el.local_name() == &markup5ever::local_name!("html")
        {
            return Err(bad_target());
        }
        if el.local_name() == &markup5ever::local_name!("script")
            && (el.is_html() || el.namespace() == &markup5ever::ns!(svg))
        {
            return Err(JsError::dom("NotSupportedError", "Cannot stream HTML into a script element"));
        }
        let before = match position {
            Position::Prepend => d.first_child(target).js()?,
            Position::Before => Some(n.id),
            Position::After | Position::Replace => d.next_sibling(n.id).js()?,
            _ => None,
        };
        (context, target, before)
    };
    let parser = StreamingFragmentParser::with_filter(page.dom.clone(), context, target, before, options.run_scripts, options.filter).js()?;
    let state = Rc::new(State {
        page: page.clone(),
        realm: s.realm().downgrade(),
        parser: RefCell::new(Some(parser)),
        alive: Rc::new(Cell::new(true)),
        closed: Cell::new(false),
        driving: Cell::new(false),
        abort_requested: Cell::new(false),
        queued: RefCell::default(),
        running_deferred: Cell::new(false),
        deferred: RefCell::default(),
        close: RefCell::default(),
    });
    let sink = s.object();
    let st = state.clone();
    sink.set_atom(
        "write",
        s.function("write", move |s, _, a| {
            let result = dom_string(a.get(0)).and_then(|text| st.drive(s, a.get(1), Some(text)));
            if result.is_err() {
                if st.driving.get() {
                    st.abort_requested.set(true);
                } else {
                    st.stop();
                }
            }
            result.map(|_| s.undefined())
        })?
        .value(),
    )?;
    let st = state.clone();
    sink.set_atom(
        "close",
        s.function("close", move |s, _, _| {
            if !st.driving.get() {
                let finished = st.close_parser()?;
                if finished {
                    st.finish();
                    return Ok(s.undefined());
                }
            }
            let d = s.deferred()?;
            let result = d.promise.get(s);
            *st.close.borrow_mut() = Some(d);
            Ok(result)
        })?
        .value(),
    )?;
    sink.set_atom(
        "abort",
        s.function("abort", move |s, _, _| {
            if state.driving.get() {
                state.abort_requested.set(true);
            } else {
                state.stop();
            }
            Ok(s.undefined())
        })?
        .value(),
    )?;
    let ctor = crate::streams::module(s, page)?.get_atom("WritableStream")?.as_object().unwrap();
    let stream = ctor.construct(&[sink.value()])?;
    if position == Position::ReplaceChildren {
        crate::input::blur_before_removal(page, target, true);
        page.dom.borrow_mut().replace_all(target, None).js()?;
    } else if position == Position::Replace {
        crate::input::blur_before_removal(page, n.id, false);
        page.dom.borrow_mut().remove_child(target, n.id).js()?;
    }
    Ok(stream.value())
}
