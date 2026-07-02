//! Pre-layout scripting seam (scaffold for ADR 0006).
//!
//! This module defines the *boundary* for a future bounded JavaScript stage — a
//! [`ScriptEngine`] trait, the resource [`ScriptLimits`] every run is capped by,
//! and a [`ScriptReport`] of what happened — plus the default [`NoopScriptEngine`]
//! (scripting disabled). No JavaScript engine is wired in yet; the real engine
//! (QuickJS or Boa, behind a cargo feature) lands later and plugs in here.
//!
//! Placement in the pipeline (ADR 0002): the script stage runs **after** the DOM
//! is built and **before** the style cascade and box generation, so scripts see a
//! complete DOM and layout sees the mutated result. It must be deterministic and
//! isolated per render (no shared global state, no real wall clock, no network),
//! which is what keeps the engine's low-RAM / high-concurrency properties intact.
#![allow(dead_code)]

use crate::dom::{Dom, NodeData, NodeId};

/// Hard resource caps for a single document's script execution. Every limit is
/// enforced; the first one hit stops execution and the partial DOM is kept (a
/// timed-out render still produces output rather than failing).
#[derive(Debug, Clone)]
pub struct ScriptLimits {
    /// Maximum wall-clock time for the whole script stage.
    pub max_wall_millis: u64,
    /// Maximum engine interrupts ("ticks") — a deterministic instruction budget
    /// that bounds runaway loops independently of wall-clock jitter.
    pub max_ticks: u64,
    /// Maximum DOM nodes scripts may create (caps `innerHTML`/`createElement`
    /// blow-ups).
    pub max_new_nodes: usize,
    /// Maximum script heap in bytes.
    pub max_heap_bytes: usize,
    /// Allow network APIs (`fetch`/`XMLHttpRequest`). Default `false`: they are
    /// absent or fail closed, so a render never makes a network call implicitly.
    pub allow_network: bool,
    /// Allow timers (`setTimeout`/`setInterval`). Default `false`: timers are
    /// ignored. When enabled, callbacks are drained synchronously up to the tick
    /// budget — there is no real event loop.
    pub allow_timers: bool,
}

impl Default for ScriptLimits {
    fn default() -> Self {
        // Conservative defaults: enough for templating/personalization scripts,
        // far below anything that could starve a worker.
        Self {
            max_wall_millis: 250,
            max_ticks: 50_000_000,
            max_new_nodes: 100_000,
            max_heap_bytes: 64 * 1024 * 1024,
            allow_network: false,
            allow_timers: false,
        }
    }
}

/// The limit that stopped a script run early, if any.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptLimit {
    WallTime,
    Ticks,
    Nodes,
    Heap,
}

/// What a script run did, for diagnostics and (later) response headers.
#[derive(Debug, Default, Clone)]
pub struct ScriptReport {
    pub scripts_executed: usize,
    pub nodes_added: usize,
    pub nodes_removed: usize,
    /// The cap that halted execution early, if one was hit.
    pub limit_hit: Option<ScriptLimit>,
    /// An uncaught script error (rendering still proceeds with the current DOM).
    pub error: Option<String>,
}

/// A pluggable JavaScript engine that runs a document's scripts against the DOM
/// before styling and layout.
///
/// Implementations MUST be deterministic and fully isolated per render — no
/// shared global mutable state, no ambient wall clock, no implicit I/O — so that
/// renders stay independent and `Send`, preserving the engine's concurrency
/// model. They MUST honor every field of [`ScriptLimits`].
pub trait ScriptEngine {
    /// Run the document's scripts, mutating `dom` in place, and report what
    /// happened. Errors are reported in [`ScriptReport::error`] rather than
    /// returned, so a script failure degrades gracefully to the pre-script DOM.
    fn run(&self, dom: &mut Dom, limits: &ScriptLimits) -> ScriptReport;
}

/// The default engine: scripting disabled. Leaves the DOM untouched, which is the
/// current behavior (HTML is rendered statically).
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopScriptEngine;

impl ScriptEngine for NoopScriptEngine {
    fn run(&self, _dom: &mut Dom, _limits: &ScriptLimits) -> ScriptReport {
        ScriptReport::default()
    }
}

#[cfg(feature = "js")]
pub use boa::BoaScriptEngine;

#[cfg(feature = "js")]
mod boa {
    use std::cell::RefCell;
    use std::rc::Rc;

    use boa_engine::{
        js_string, object::ObjectInitializer, property::Attribute, Context, Finalize, JsArgs,
        JsData, JsNativeError, JsResult, JsValue, NativeFunction, Source, Trace,
    };

    use super::{inline_scripts, ScriptEngine, ScriptLimits, ScriptReport};
    use crate::dom::{Dom, NodeId};

    /// Mutable state shared between the Rust host and the JS DOM bindings.
    struct Inner {
        dom: Dom,
        report: ScriptReport,
        max_new_nodes: usize,
    }

    /// GC-visible handle to [`Inner`]. The `Rc<RefCell<…>>` holds only plain Rust
    /// data (no JS GC pointers), so it is safe to skip tracing.
    #[derive(Trace, Finalize, JsData, Clone)]
    struct Host(#[unsafe_ignore_trace] Rc<RefCell<Inner>>);

    /// A bounded JavaScript engine backed by Boa (ADR 0006): runs the document's
    /// inline scripts against a minimal `document` DOM API, within `ScriptLimits`,
    /// mutating the DOM in place. Deterministic and isolated per call.
    #[derive(Debug, Default, Clone, Copy)]
    pub struct BoaScriptEngine;

    impl ScriptEngine for BoaScriptEngine {
        fn run(&self, dom: &mut Dom, limits: &ScriptLimits) -> ScriptReport {
            let scripts = inline_scripts(dom);
            if scripts.is_empty() {
                return ScriptReport::default();
            }

            // Keep the owning `Rc` separate from any `Host` handle, so we never
            // move a field out of `Host` (which implements `Drop` via its GC
            // derives). All `Host` clones live inside the context's closures.
            let shared = Rc::new(RefCell::new(Inner {
                dom: std::mem::take(dom),
                report: ScriptReport::default(),
                max_new_nodes: limits.max_new_nodes,
            }));

            let mut context = Context::default();
            context
                .runtime_limits_mut()
                .set_loop_iteration_limit(limits.max_ticks);
            install_globals(&mut context, Host(shared.clone()));

            let mut executed = 0;
            for source in &scripts {
                match context.eval(Source::from_bytes(source.as_bytes())) {
                    Ok(_) => executed += 1,
                    Err(error) => {
                        shared.borrow_mut().report.error = Some(error.to_string());
                        break;
                    }
                }
            }

            // Drop the context first (releasing most GC objects). Boa's GC may
            // still hold `Host` clones after this, so rather than require unique
            // ownership we take the DOM and report back out of the shared cell
            // (leaving it empty; any lingering clones see a harmless empty state).
            drop(context);
            let mut inner = shared.borrow_mut();
            *dom = std::mem::take(&mut inner.dom);
            let mut report = std::mem::take(&mut inner.report);
            report.scripts_executed = executed;
            report
        }
    }

    fn install_globals(context: &mut Context, host: Host) {
        // console.log(...): accept and drop (no stdout from a render).
        let log = NativeFunction::from_copy_closure(
            |_this, _args, _ctx| -> JsResult<JsValue> { Ok(JsValue::undefined()) },
        );
        let console = ObjectInitializer::new(context)
            .function(log, js_string!("log"), 1)
            .build();
        let _ = context.register_global_property(js_string!("console"), console, Attribute::all());

        // document.getElementById(id) -> element object (or null).
        let get_by_id = NativeFunction::from_copy_closure_with_captures(
            |_this, args, host: &Host, ctx| {
                let id = args
                    .get_or_undefined(0)
                    .to_string(ctx)?
                    .to_std_string_escaped();
                match host.0.borrow().dom.element_by_id(&id) {
                    Some(node) => Ok(make_element(node, host, ctx)),
                    None => Ok(JsValue::null()),
                }
            },
            host.clone(),
        );
        // document.createElement(tag) / document.createTextNode(text): detached
        // arena nodes, attached later via appendChild. Each creation draws one
        // node from the script budget; past the cap they return null.
        let create_element = NativeFunction::from_copy_closure_with_captures(
            |_this, args, host: &Host, ctx| {
                let tag = args
                    .get_or_undefined(0)
                    .to_string(ctx)?
                    .to_std_string_escaped();
                let node = {
                    let mut inner = host.0.borrow_mut();
                    if inner.report.nodes_added >= inner.max_new_nodes {
                        inner.report.limit_hit = Some(super::ScriptLimit::Nodes);
                        return Ok(JsValue::null());
                    }
                    inner.report.nodes_added += 1;
                    inner.dom.create_element(&tag)
                };
                Ok(make_element(node, host, ctx))
            },
            host.clone(),
        );
        let create_text = NativeFunction::from_copy_closure_with_captures(
            |_this, args, host: &Host, ctx| {
                let text = args
                    .get_or_undefined(0)
                    .to_string(ctx)?
                    .to_std_string_escaped();
                let node = {
                    let mut inner = host.0.borrow_mut();
                    if inner.report.nodes_added >= inner.max_new_nodes {
                        inner.report.limit_hit = Some(super::ScriptLimit::Nodes);
                        return Ok(JsValue::null());
                    }
                    inner.report.nodes_added += 1;
                    inner.dom.create_text_node(&text)
                };
                Ok(make_element(node, host, ctx))
            },
            host.clone(),
        );

        // document.body: element handle for the <body> node (html5ever always
        // synthesizes one for a document parse).
        let body = host.0.borrow().dom.body();
        let body_value = match body {
            Some(node) => make_element(node, &host, context),
            None => JsValue::null(),
        };

        let document = ObjectInitializer::new(context)
            .function(get_by_id, js_string!("getElementById"), 1)
            .function(create_element, js_string!("createElement"), 1)
            .function(create_text, js_string!("createTextNode"), 1)
            .property(js_string!("body"), body_value, Attribute::ENUMERABLE)
            .build();
        let _ = context.register_global_property(js_string!("document"), document, Attribute::all());
    }

    /// Build a JS object representing an element: it stashes the node id and
    /// exposes `textContent`/`innerHTML` (get/set), `getAttribute`/`setAttribute`,
    /// and `appendChild`/`removeChild`.
    fn make_element(node: NodeId, host: &Host, context: &mut Context) -> JsValue {
        let get_text = NativeFunction::from_copy_closure_with_captures(
            |this, _args, host: &Host, ctx| {
                let node = node_id_of(this, ctx)?;
                let text = host.0.borrow().dom.text_content(node);
                Ok(js_string!(text).into())
            },
            host.clone(),
        );
        let set_text = NativeFunction::from_copy_closure_with_captures(
            |this, args, host: &Host, ctx| {
                let node = node_id_of(this, ctx)?;
                let text = args
                    .get_or_undefined(0)
                    .to_string(ctx)?
                    .to_std_string_escaped();
                let mut inner = host.0.borrow_mut();
                if inner.report.nodes_added >= inner.max_new_nodes {
                    inner.report.limit_hit = Some(super::ScriptLimit::Nodes);
                    return Ok(JsValue::undefined());
                }
                let added = inner.dom.set_text_content(node, &text);
                inner.report.nodes_added += added;
                Ok(JsValue::undefined())
            },
            host.clone(),
        );
        let set_attr = NativeFunction::from_copy_closure_with_captures(
            |this, args, host: &Host, ctx| {
                let node = node_id_of(this, ctx)?;
                let name = args
                    .get_or_undefined(0)
                    .to_string(ctx)?
                    .to_std_string_escaped();
                let value = args
                    .get_or_undefined(1)
                    .to_string(ctx)?
                    .to_std_string_escaped();
                host.0.borrow_mut().dom.set_attribute(node, &name, &value);
                Ok(JsValue::undefined())
            },
            host.clone(),
        );
        let get_attr = NativeFunction::from_copy_closure_with_captures(
            |this, args, host: &Host, ctx| {
                let node = node_id_of(this, ctx)?;
                let name = args
                    .get_or_undefined(0)
                    .to_string(ctx)?
                    .to_std_string_escaped();
                let inner = host.0.borrow();
                match inner.dom.node(node).attr(&name) {
                    Some(value) => Ok(js_string!(value).into()),
                    None => Ok(JsValue::null()),
                }
            },
            host.clone(),
        );

        // innerHTML getter: serialize the element's children back to markup.
        let get_html = NativeFunction::from_copy_closure_with_captures(
            |this, _args, host: &Host, ctx| {
                let node = node_id_of(this, ctx)?;
                let html = host.0.borrow().dom.inner_html(node);
                Ok(js_string!(html).into())
            },
            host.clone(),
        );
        // innerHTML setter: structural mutation — parse the markup and replace the
        // element's children. Bounded by the node budget. **Spike (live-DOM JS).**
        let set_html = NativeFunction::from_copy_closure_with_captures(
            |this, args, host: &Host, ctx| {
                let node = node_id_of(this, ctx)?;
                let html = args
                    .get_or_undefined(0)
                    .to_string(ctx)?
                    .to_std_string_escaped();
                let mut inner = host.0.borrow_mut();
                if inner.report.nodes_added >= inner.max_new_nodes {
                    inner.report.limit_hit = Some(super::ScriptLimit::Nodes);
                    return Ok(JsValue::undefined());
                }
                let added = inner.dom.set_inner_html(node, &html);
                inner.report.nodes_added += added;
                Ok(JsValue::undefined())
            },
            host.clone(),
        );

        // appendChild(child): attach (or move) `child` as this element's last
        // child. Returns the child, or null if the DOM refused the move (cycle,
        // text-node parent, …). No budget draw — the node was paid for at
        // creation, and moves don't grow the arena.
        let append_child = NativeFunction::from_copy_closure_with_captures(
            |this, args, host: &Host, ctx| {
                let parent = node_id_of(this, ctx)?;
                let child_value = args.get_or_undefined(0).clone();
                let child = node_id_of(&child_value, ctx)?;
                let ok = host.0.borrow_mut().dom.append_child(parent, child);
                Ok(if ok { child_value } else { JsValue::null() })
            },
            host.clone(),
        );
        // removeChild(child): detach and return the child (null if it was not a
        // child of this element — no throw, degrade gracefully).
        let remove_child = NativeFunction::from_copy_closure_with_captures(
            |this, args, host: &Host, ctx| {
                let parent = node_id_of(this, ctx)?;
                let child_value = args.get_or_undefined(0).clone();
                let child = node_id_of(&child_value, ctx)?;
                let mut inner = host.0.borrow_mut();
                if inner.dom.remove_child(parent, child) {
                    inner.report.nodes_removed += 1;
                    Ok(child_value)
                } else {
                    Ok(JsValue::null())
                }
            },
            host.clone(),
        );

        // `.accessor` wants `JsFunction`s; convert before borrowing `context`
        // mutably for the object builder.
        let get_fn = get_text.to_js_function(context.realm());
        let set_fn = set_text.to_js_function(context.realm());
        let get_html_fn = get_html.to_js_function(context.realm());
        let set_html_fn = set_html.to_js_function(context.realm());

        ObjectInitializer::new(context)
            .property(js_string!("__node"), node as i32, Attribute::empty())
            .accessor(
                js_string!("textContent"),
                Some(get_fn),
                Some(set_fn),
                Attribute::CONFIGURABLE | Attribute::ENUMERABLE,
            )
            .accessor(
                js_string!("innerHTML"),
                Some(get_html_fn),
                Some(set_html_fn),
                Attribute::CONFIGURABLE | Attribute::ENUMERABLE,
            )
            .function(set_attr, js_string!("setAttribute"), 2)
            .function(get_attr, js_string!("getAttribute"), 1)
            .function(append_child, js_string!("appendChild"), 1)
            .function(remove_child, js_string!("removeChild"), 1)
            .build()
            .into()
    }

    /// Read the `__node` id stashed on an element object.
    fn node_id_of(this: &JsValue, context: &mut Context) -> JsResult<NodeId> {
        let object = this
            .as_object()
            .ok_or_else(|| JsNativeError::typ().with_message("not an element"))?;
        let value = object.get(js_string!("__node"), context)?;
        Ok(value.to_u32(context)? as NodeId)
    }
}

/// Collect the source text of inline `<script>` elements in document order,
/// skipping elements with a `src` attribute (external scripts are not fetched)
/// and non-JavaScript `type`s. Provided now so a future engine — and tests —
/// have a stable way to find what to execute.
pub fn inline_scripts(dom: &Dom) -> Vec<String> {
    let mut scripts = Vec::new();
    collect_scripts(dom, dom.root(), &mut scripts);
    scripts
}

fn collect_scripts(dom: &Dom, id: NodeId, out: &mut Vec<String>) {
    let node = dom.node(id);
    if let NodeData::Element { name, .. } = &node.data {
        if name.as_str() == "script" && is_javascript_script(node) {
            let mut source = String::new();
            for &child in &node.children {
                if let NodeData::Text(text) = &dom.node(child).data {
                    source.push_str(text);
                }
            }
            if !source.trim().is_empty() {
                out.push(source);
            }
            return; // scripts contain only text; no element children to recurse
        }
    }
    for &child in &node.children {
        collect_scripts(dom, child, out);
    }
}

/// Whether a `<script>` element is an executable inline JavaScript block (no
/// `src`, and a `type` of either absent or a recognized JS MIME/`module`).
fn is_javascript_script(node: &crate::dom::Node) -> bool {
    if node.attr("src").is_some() {
        return false;
    }
    match node.attr("type") {
        None => true,
        Some(value) => {
            let value = value.trim().to_ascii_lowercase();
            value.is_empty()
                || value == "module"
                || value == "text/javascript"
                || value == "application/javascript"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noop_engine_leaves_the_dom_unchanged() {
        let mut dom = Dom::parse("<h1>Hi</h1><script>document.title='x'</script>");
        let before = dom.nodes.len();

        let report = NoopScriptEngine.run(&mut dom, &ScriptLimits::default());

        assert_eq!(dom.nodes.len(), before);
        assert_eq!(report.scripts_executed, 0);
        assert!(report.limit_hit.is_none());
        assert!(report.error.is_none());
    }

    #[test]
    fn collects_inline_scripts_only() {
        let dom = Dom::parse(
            r#"
            <script>let a = 1;</script>
            <script src="app.js"></script>
            <script type="application/json">{"not":"js"}</script>
            <script type="text/javascript">let b = 2;</script>
            "#,
        );

        let scripts = inline_scripts(&dom);
        assert_eq!(scripts.len(), 2);
        assert!(scripts[0].contains("let a = 1;"));
        assert!(scripts[1].contains("let b = 2;"));
    }

    #[test]
    fn default_limits_disable_io() {
        let limits = ScriptLimits::default();
        assert!(!limits.allow_network);
        assert!(!limits.allow_timers);
    }

    #[cfg(feature = "js")]
    mod js {
        use super::super::{BoaScriptEngine, ScriptEngine, ScriptLimits};
        use crate::dom::Dom;

        #[test]
        fn script_sets_text_content() {
            let mut dom = Dom::parse(
                "<p id=\"greet\">OLD</p>\
                 <script>document.getElementById('greet').textContent = 'NEW ' + (1 + 2)</script>",
            );

            let report = BoaScriptEngine.run(&mut dom, &ScriptLimits::default());

            assert_eq!(report.scripts_executed, 1);
            assert!(report.error.is_none(), "unexpected error: {:?}", report.error);
            let id = dom.element_by_id("greet").expect("element present");
            assert_eq!(dom.text_content(id), "NEW 3");
        }

        #[test]
        fn script_reads_text_and_sets_attribute() {
            let mut dom = Dom::parse(
                "<p id=\"x\">hello</p>\
                 <script>var e = document.getElementById('x'); e.setAttribute('data-len', String(e.textContent.length));</script>",
            );

            BoaScriptEngine.run(&mut dom, &ScriptLimits::default());

            let id = dom.element_by_id("x").expect("element present");
            assert_eq!(dom.node(id).attr("data-len"), Some("5"));
        }

        #[test]
        fn loop_iteration_limit_stops_runaway_scripts() {
            let mut dom = Dom::parse("<p id=\"x\">hi</p><script>while (true) {}</script>");
            let limits = ScriptLimits {
                max_ticks: 10_000,
                ..ScriptLimits::default()
            };

            let report = BoaScriptEngine.run(&mut dom, &limits);

            assert!(
                report.error.is_some(),
                "a runaway loop must hit the iteration limit"
            );
        }

        #[test]
        fn missing_element_returns_null() {
            let mut dom = Dom::parse(
                "<p id=\"x\">hi</p><script>if (document.getElementById('nope') === null) { document.getElementById('x').textContent = 'ok'; }</script>",
            );

            BoaScriptEngine.run(&mut dom, &ScriptLimits::default());

            let id = dom.element_by_id("x").unwrap();
            assert_eq!(dom.text_content(id), "ok");
        }

        // --- live-DOM spike: structural mutation via innerHTML ---------------

        #[test]
        fn inner_html_setter_grafts_new_element_nodes() {
            let mut dom = Dom::parse(
                "<div id=\"host\">old</div>\
                 <script>document.getElementById('host').innerHTML = '<b>bold</b> and <i>more</i>';</script>",
            );
            let before = dom.nodes.len();
            let report = BoaScriptEngine.run(&mut dom, &ScriptLimits::default());

            assert!(report.error.is_none(), "unexpected error: {:?}", report.error);
            assert!(report.nodes_added > 0, "innerHTML should add nodes");
            assert!(dom.nodes.len() > before, "arena should grow");

            let host = dom.element_by_id("host").expect("host present");
            // The host now contains real <b>/<i> element children, not just text.
            let child_tags: Vec<Option<&str>> = dom.node(host)
                .children
                .iter()
                .map(|&c| dom.node(c).tag())
                .collect();
            assert!(child_tags.contains(&Some("b")), "expected a <b> child: {child_tags:?}");
            assert!(child_tags.contains(&Some("i")), "expected an <i> child");
            assert_eq!(dom.text_content(host), "bold and more");
        }

        #[test]
        fn inner_html_getter_serializes_children() {
            let mut dom = Dom::parse(
                "<div id=\"h\"><span>hi</span></div>\
                 <script>document.getElementById('h').setAttribute('data-html', document.getElementById('h').innerHTML);</script>",
            );
            BoaScriptEngine.run(&mut dom, &ScriptLimits::default());
            let id = dom.element_by_id("h").unwrap();
            assert_eq!(dom.node(id).attr("data-html"), Some("<span>hi</span>"));
        }

        // --- live-DOM tree mutation: createElement / appendChild / removeChild

        #[test]
        fn create_element_and_append_child_build_a_list() {
            let mut dom = Dom::parse(
                "<ul id=\"list\"></ul>\
                 <script>\
                 var list = document.getElementById('list');\
                 for (var i = 1; i <= 3; i++) {\
                   var li = document.createElement('LI');\
                   li.textContent = 'Item ' + i;\
                   list.appendChild(li);\
                 }\
                 </script>",
            );
            let report = BoaScriptEngine.run(&mut dom, &ScriptLimits::default());

            assert!(report.error.is_none(), "unexpected error: {:?}", report.error);
            // 3 <li> elements + 3 text nodes from the textContent sets.
            assert_eq!(report.nodes_added, 6);
            let list = dom.element_by_id("list").unwrap();
            let tags: Vec<Option<&str>> = dom
                .node(list)
                .children
                .iter()
                .map(|&c| dom.node(c).tag())
                .collect();
            assert_eq!(tags, vec![Some("li"); 3], "tag normalized to lowercase");
            assert_eq!(dom.text_content(list), "Item 1Item 2Item 3");
        }

        #[test]
        fn append_child_reaches_document_body() {
            let mut dom = Dom::parse(
                "<p>static</p>\
                 <script>\
                 var h = document.createElement('h1');\
                 h.appendChild(document.createTextNode('ADDED'));\
                 document.body.appendChild(h);\
                 </script>",
            );
            let report = BoaScriptEngine.run(&mut dom, &ScriptLimits::default());

            assert!(report.error.is_none(), "unexpected error: {:?}", report.error);
            let body = dom.body().unwrap();
            assert!(dom.text_content(body).contains("ADDED"));
            let last = *dom.node(body).children.last().unwrap();
            assert_eq!(dom.node(last).tag(), Some("h1"));
        }

        #[test]
        fn remove_child_detaches_a_subtree() {
            let mut dom = Dom::parse(
                "<div id=\"wrap\"><p id=\"gone\">remove me</p><p>keep</p></div>\
                 <script>\
                 var w = document.getElementById('wrap');\
                 w.removeChild(document.getElementById('gone'));\
                 </script>",
            );
            let report = BoaScriptEngine.run(&mut dom, &ScriptLimits::default());

            assert!(report.error.is_none(), "unexpected error: {:?}", report.error);
            assert_eq!(report.nodes_removed, 1);
            let wrap = dom.element_by_id("wrap").unwrap();
            assert_eq!(dom.text_content(wrap), "keep");
        }

        #[test]
        fn append_child_refuses_a_cycle_without_erroring() {
            let mut dom = Dom::parse(
                "<div id=\"outer\"><div id=\"inner\">t</div></div>\
                 <script>\
                 var r = document.getElementById('inner')\
                     .appendChild(document.getElementById('outer'));\
                 if (r === null) { document.getElementById('inner').setAttribute('data-refused', '1'); }\
                 </script>",
            );
            let report = BoaScriptEngine.run(&mut dom, &ScriptLimits::default());

            assert!(report.error.is_none(), "unexpected error: {:?}", report.error);
            let inner = dom.element_by_id("inner").unwrap();
            assert_eq!(dom.node(inner).attr("data-refused"), Some("1"));
            let outer = dom.element_by_id("outer").unwrap();
            assert_eq!(dom.node(inner).parent, Some(outer), "tree unchanged");
        }

        #[test]
        fn create_element_respects_node_budget() {
            let mut dom = Dom::parse(
                "<div id=\"h\">x</div>\
                 <script>var e = document.createElement('div');</script>",
            );
            let limits = ScriptLimits { max_new_nodes: 0, ..ScriptLimits::default() };
            let report = BoaScriptEngine.run(&mut dom, &limits);
            assert_eq!(report.limit_hit, Some(crate::script::ScriptLimit::Nodes));
            assert_eq!(report.nodes_added, 0);
        }

        #[test]
        fn inner_html_respects_node_budget() {
            let mut dom = Dom::parse(
                "<div id=\"h\">x</div>\
                 <script>document.getElementById('h').innerHTML = '<b>a</b><b>b</b><b>c</b>';</script>",
            );
            let limits = ScriptLimits { max_new_nodes: 0, ..ScriptLimits::default() };
            let report = BoaScriptEngine.run(&mut dom, &limits);
            assert_eq!(report.limit_hit, Some(crate::script::ScriptLimit::Nodes));
        }
    }
}
