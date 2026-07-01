//! A compact, arena-backed DOM.
//!
//! HTML is parsed by `html5ever` (the spec-compliant Servo tokenizer and tree
//! builder). Rather than route through `markup5ever_rcdom`'s `Rc`/`RefCell`
//! tree and then copy it, we implement `html5ever`'s [`TreeSink`] directly
//! against a flat `Vec` arena. This removes the `Rc` graph entirely: there is no
//! per-node reference counting, no `Weak` parent cells, and no second tree held
//! in memory at the same time. The result is a cache-friendly, low-overhead,
//! `Send` structure, which keeps each render independent and parallelizable and
//! keeps peak parse-time memory low (ADR 0002).
//!
//! The sink stores every node kind the tree builder can create (including
//! comments, doctypes, and processing instructions, which it must be able to
//! reference during construction). [`TreeSink::finish`] lowers that into the
//! public [`Dom`], which keeps only the rendered node kinds.

use std::borrow::Cow;
use std::cell::{Cell, RefCell};

use html5ever::tendril::{StrTendril, TendrilSink};
use html5ever::tree_builder::{
    Attribute, ElemName, ElementFlags, NodeOrText, QuirksMode, TreeSink,
};
use html5ever::{LocalName, Namespace, QualName};

/// Index of a node within [`Dom::nodes`].
pub type NodeId = usize;

/// The whole document as a flat arena. Node `0` is always the document root.
/// `Default` is an empty arena, used only as a transient placeholder while the
/// scripting stage owns the real DOM.
#[derive(Debug, Clone, Default)]
pub struct Dom {
    pub nodes: Vec<Node>,
}

#[derive(Debug, Clone)]
pub struct Node {
    // Read by the cascade's inheritance walk (ADR 0002 step 5), which lands
    // next; allowed until that caller exists.
    #[allow(dead_code)]
    pub parent: Option<NodeId>,
    pub children: Vec<NodeId>,
    pub data: NodeData,
}

#[derive(Debug, Clone)]
pub enum NodeData {
    Document,
    Element {
        /// Lowercased local tag name (html5ever normalizes HTML element names).
        name: String,
        attrs: Vec<(String, String)>,
    },
    Text(String),
}

impl Dom {
    /// Parse an HTML document string into the arena DOM.
    pub fn parse(input: &str) -> Dom {
        html5ever::parse_document(ArenaSink::new(), Default::default())
            .from_utf8()
            .read_from(&mut input.as_bytes())
            // Reading from an in-memory byte slice is infallible.
            .expect("in-memory HTML parsing cannot fail")
    }

    /// The document root node id (always `0`).
    pub fn root(&self) -> NodeId {
        0
    }

    pub fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id]
    }
}

/// Minimal DOM mutation surface used by the pre-layout scripting stage (ADR
/// 0006). These are the primitives a `ScriptEngine`'s DOM bindings call; they are
/// deliberately small and index-based to match the arena model.
#[allow(dead_code)]
impl Dom {
    /// The first element whose `id` attribute equals `value` (document order).
    pub fn element_by_id(&self, value: &str) -> Option<NodeId> {
        self.nodes.iter().position(|node| {
            matches!(node.data, NodeData::Element { .. }) && node.attr("id") == Some(value)
        })
    }

    /// The concatenated text of a node's subtree (like DOM `textContent`).
    pub fn text_content(&self, id: NodeId) -> String {
        let mut out = String::new();
        self.collect_text_content(id, &mut out);
        out
    }

    fn collect_text_content(&self, id: NodeId, out: &mut String) {
        match &self.nodes[id].data {
            NodeData::Text(text) => out.push_str(text),
            _ => {
                for index in 0..self.nodes[id].children.len() {
                    let child = self.nodes[id].children[index];
                    self.collect_text_content(child, out);
                }
            }
        }
    }

    /// Set a node's `textContent`: for a text node, replace its string; for an
    /// element, replace its children with a single text node. Returns the number
    /// of nodes added (0 or 1), for resource accounting. Old children are left
    /// orphaned in the arena (unreachable from the root), which is harmless.
    pub fn set_text_content(&mut self, id: NodeId, text: &str) -> usize {
        if let NodeData::Text(existing) = &mut self.nodes[id].data {
            *existing = text.to_string();
            return 0;
        }
        let text_id = self.nodes.len();
        self.nodes.push(Node {
            parent: Some(id),
            children: Vec::new(),
            data: NodeData::Text(text.to_string()),
        });
        self.nodes[id].children = vec![text_id];
        1
    }

    /// Replace a node's children with the parsed markup `html` (like DOM
    /// `innerHTML =`). Structural mutation: the fragment is parsed into a scratch
    /// DOM and its `<body>` children are grafted into this arena under `id`. Old
    /// children are orphaned (harmless). Returns the number of nodes added, for
    /// the script node budget. **Spike (live-DOM JS).**
    pub fn set_inner_html(&mut self, id: NodeId, html: &str) -> usize {
        let fragment = Dom::parse(html);
        let Some(body) = fragment
            .nodes
            .iter()
            .position(|node| node.tag() == Some("body"))
        else {
            return 0;
        };
        let before = self.nodes.len();
        let src_children = fragment.nodes[body].children.clone();
        let mut new_children = Vec::with_capacity(src_children.len());
        for src_child in src_children {
            new_children.push(self.graft(&fragment, src_child, id));
        }
        self.nodes[id].children = new_children;
        self.nodes.len() - before
    }

    /// Deep-copy `src_id` (and its subtree) from another DOM's arena into this
    /// one under `parent`, returning the new node id. Ids are remapped as nodes
    /// are appended.
    fn graft(&mut self, src: &Dom, src_id: NodeId, parent: NodeId) -> NodeId {
        let new_id = self.nodes.len();
        self.nodes.push(Node {
            parent: Some(parent),
            children: Vec::new(),
            data: src.nodes[src_id].data.clone(),
        });
        let src_children = src.nodes[src_id].children.clone();
        let mut kids = Vec::with_capacity(src_children.len());
        for src_child in src_children {
            kids.push(self.graft(src, src_child, new_id));
        }
        self.nodes[new_id].children = kids;
        new_id
    }

    /// Serialize a node's children back to HTML (like DOM `innerHTML` getter).
    /// Minimal (no attribute-value escaping) — enough for the scripting spike.
    pub fn inner_html(&self, id: NodeId) -> String {
        let mut out = String::new();
        for &child in &self.nodes[id].children {
            self.serialize_node(child, &mut out);
        }
        out
    }

    fn serialize_node(&self, id: NodeId, out: &mut String) {
        match &self.nodes[id].data {
            NodeData::Text(text) => out.push_str(text),
            NodeData::Element { name, attrs } => {
                out.push('<');
                out.push_str(name);
                for (key, value) in attrs {
                    out.push(' ');
                    out.push_str(key);
                    out.push_str("=\"");
                    out.push_str(value);
                    out.push('"');
                }
                out.push('>');
                for &child in &self.nodes[id].children {
                    self.serialize_node(child, out);
                }
                out.push_str("</");
                out.push_str(name);
                out.push('>');
            }
            NodeData::Document => {
                for &child in &self.nodes[id].children {
                    self.serialize_node(child, out);
                }
            }
        }
    }

    /// Set (or add) an attribute on an element node; no-op for non-elements.
    pub fn set_attribute(&mut self, id: NodeId, name: &str, value: &str) {
        if let NodeData::Element { attrs, .. } = &mut self.nodes[id].data {
            if let Some(entry) = attrs.iter_mut().find(|(key, _)| key == name) {
                entry.1 = value.to_string();
            } else {
                attrs.push((name.to_string(), value.to_string()));
            }
        }
    }
}

// Read accessors for the cascade/box-tree work that consumes this DOM next
// (ADR 0002 migration steps 4-6). Kept on the type now so the DOM is the single
// query surface; `dead_code` is allowed until those callers land.
#[allow(dead_code)]
impl Node {
    /// The element's lowercased tag name, if this node is an element.
    pub fn tag(&self) -> Option<&str> {
        match &self.data {
            NodeData::Element { name, .. } => Some(name),
            _ => None,
        }
    }

    /// Value of an attribute (case-insensitive name), if present.
    pub fn attr(&self, name: &str) -> Option<&str> {
        match &self.data {
            NodeData::Element { attrs, .. } => attrs
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.as_str()),
            _ => None,
        }
    }

    /// Whitespace-separated tokens of the `class` attribute.
    pub fn classes(&self) -> impl Iterator<Item = &str> {
        self.attr("class").unwrap_or_default().split_whitespace()
    }
}

// ---------------------------------------------------------------------------
// Custom html5ever TreeSink that builds the arena directly (no RcDom).
// ---------------------------------------------------------------------------

type SinkId = usize;

enum SinkData {
    Document,
    Doctype,
    Comment,
    ProcessingInstruction,
    Text(String),
    Element {
        name: QualName,
        attrs: Vec<Attribute>,
        /// Content document for a `<template>` element, if any.
        template: Option<SinkId>,
        mathml_integration_point: bool,
    },
}

struct SinkNode {
    parent: Option<SinkId>,
    children: Vec<SinkId>,
    data: SinkData,
}

struct ArenaSink {
    nodes: RefCell<Vec<SinkNode>>,
    quirks: Cell<QuirksMode>,
}

/// Owned element name so `elem_name` can return a value that borrows from
/// itself, freeing the `Handle` to be a plain index instead of a reference into
/// the (interior-mutable) arena.
#[derive(Debug)]
struct OwnedElemName(QualName);

impl ElemName for OwnedElemName {
    fn ns(&self) -> &Namespace {
        &self.0.ns
    }

    fn local_name(&self) -> &LocalName {
        &self.0.local
    }
}

impl ArenaSink {
    fn new() -> Self {
        Self {
            nodes: RefCell::new(vec![SinkNode {
                parent: None,
                children: Vec::new(),
                data: SinkData::Document,
            }]),
            quirks: Cell::new(QuirksMode::NoQuirks),
        }
    }

    fn push(&self, data: SinkData) -> SinkId {
        let mut nodes = self.nodes.borrow_mut();
        let id = nodes.len();
        nodes.push(SinkNode {
            parent: None,
            children: Vec::new(),
            data,
        });
        id
    }

    fn attach(&self, parent: SinkId, child: SinkId) {
        let mut nodes = self.nodes.borrow_mut();
        nodes[child].parent = Some(parent);
        nodes[parent].children.push(child);
    }

    fn insert_at(&self, parent: SinkId, index: usize, child: SinkId) {
        let mut nodes = self.nodes.borrow_mut();
        nodes[child].parent = Some(parent);
        nodes[parent].children.insert(index, child);
    }

    fn detach(&self, target: SinkId) {
        let mut nodes = self.nodes.borrow_mut();
        if let Some(parent) = nodes[target].parent.take() {
            nodes[parent].children.retain(|&child| child != target);
        }
    }

    fn parent_and_index(&self, target: SinkId) -> Option<(SinkId, usize)> {
        let nodes = self.nodes.borrow();
        let parent = nodes[target].parent?;
        let index = nodes[parent]
            .children
            .iter()
            .position(|&child| child == target)
            .expect("node has parent but is missing from its children");
        Some((parent, index))
    }
}

impl TreeSink for ArenaSink {
    type Handle = SinkId;
    type Output = Dom;
    type ElemName<'a>
        = OwnedElemName
    where
        Self: 'a;

    fn finish(self) -> Dom {
        let sink = self.nodes.into_inner();
        let mut dom = Dom {
            nodes: vec![Node {
                parent: None,
                children: Vec::new(),
                data: NodeData::Document,
            }],
        };
        lower(&sink, 0, 0, &mut dom);
        dom
    }

    fn parse_error(&self, _msg: Cow<'static, str>) {}

    fn get_document(&self) -> SinkId {
        0
    }

    fn elem_name<'a>(&'a self, target: &'a SinkId) -> OwnedElemName {
        match &self.nodes.borrow()[*target].data {
            SinkData::Element { name, .. } => OwnedElemName(name.clone()),
            _ => panic!("elem_name called on a non-element node"),
        }
    }

    fn create_element(
        &self,
        name: QualName,
        attrs: Vec<Attribute>,
        flags: ElementFlags,
    ) -> SinkId {
        let template = flags.template.then(|| self.push(SinkData::Document));
        self.push(SinkData::Element {
            name,
            attrs,
            template,
            mathml_integration_point: flags.mathml_annotation_xml_integration_point,
        })
    }

    fn create_comment(&self, _text: StrTendril) -> SinkId {
        self.push(SinkData::Comment)
    }

    fn create_pi(&self, _target: StrTendril, _data: StrTendril) -> SinkId {
        self.push(SinkData::ProcessingInstruction)
    }

    fn append(&self, parent: &SinkId, child: NodeOrText<SinkId>) {
        match child {
            NodeOrText::AppendText(text) => {
                // Merge with a trailing text sibling, matching the spec/RcDom.
                {
                    let mut nodes = self.nodes.borrow_mut();
                    if let Some(&last) = nodes[*parent].children.last() {
                        if let SinkData::Text(existing) = &mut nodes[last].data {
                            existing.push_str(&text);
                            return;
                        }
                    }
                }
                let id = self.push(SinkData::Text(text.to_string()));
                self.attach(*parent, id);
            }
            NodeOrText::AppendNode(id) => self.attach(*parent, id),
        }
    }

    fn append_before_sibling(&self, sibling: &SinkId, child: NodeOrText<SinkId>) {
        let (parent, index) = self
            .parent_and_index(*sibling)
            .expect("append_before_sibling on a node without a parent");

        match child {
            NodeOrText::AppendText(text) => {
                // Merge with the text node immediately before the insertion point.
                if index > 0 {
                    let mut nodes = self.nodes.borrow_mut();
                    let prev = nodes[parent].children[index - 1];
                    if let SinkData::Text(existing) = &mut nodes[prev].data {
                        existing.push_str(&text);
                        return;
                    }
                }
                let id = self.push(SinkData::Text(text.to_string()));
                self.insert_at(parent, index, id);
            }
            NodeOrText::AppendNode(id) => {
                self.detach(id);
                self.insert_at(parent, index, id);
            }
        }
    }

    fn append_based_on_parent_node(
        &self,
        element: &SinkId,
        prev_element: &SinkId,
        child: NodeOrText<SinkId>,
    ) {
        let has_parent = self.nodes.borrow()[*element].parent.is_some();
        if has_parent {
            self.append_before_sibling(element, child);
        } else {
            self.append(prev_element, child);
        }
    }

    fn append_doctype_to_document(
        &self,
        _name: StrTendril,
        _public_id: StrTendril,
        _system_id: StrTendril,
    ) {
        let id = self.push(SinkData::Doctype);
        self.attach(0, id);
    }

    fn get_template_contents(&self, target: &SinkId) -> SinkId {
        match &self.nodes.borrow()[*target].data {
            SinkData::Element {
                template: Some(contents),
                ..
            } => *contents,
            _ => panic!("get_template_contents on a non-template element"),
        }
    }

    fn same_node(&self, x: &SinkId, y: &SinkId) -> bool {
        x == y
    }

    fn set_quirks_mode(&self, mode: QuirksMode) {
        self.quirks.set(mode);
    }

    fn add_attrs_if_missing(&self, target: &SinkId, attrs: Vec<Attribute>) {
        let mut nodes = self.nodes.borrow_mut();
        if let SinkData::Element { attrs: existing, .. } = &mut nodes[*target].data {
            for attr in attrs {
                if !existing.iter().any(|present| present.name == attr.name) {
                    existing.push(attr);
                }
            }
        }
    }

    fn remove_from_parent(&self, target: &SinkId) {
        self.detach(*target);
    }

    fn reparent_children(&self, node: &SinkId, new_parent: &SinkId) {
        let mut nodes = self.nodes.borrow_mut();
        let moved = std::mem::take(&mut nodes[*node].children);
        for &child in &moved {
            nodes[child].parent = Some(*new_parent);
        }
        nodes[*new_parent].children.extend(moved);
    }

    fn is_mathml_annotation_xml_integration_point(&self, target: &SinkId) -> bool {
        matches!(
            &self.nodes.borrow()[*target].data,
            SinkData::Element {
                mathml_integration_point: true,
                ..
            }
        )
    }
}

/// Lower the sink's full tree into the public [`Dom`], keeping only rendered
/// node kinds (document, elements, text) and discarding doctypes, comments, and
/// processing instructions. Element and attribute names are lowercased to match
/// the rest of the engine.
fn lower(sink: &[SinkNode], sink_id: SinkId, dom_parent: NodeId, dom: &mut Dom) {
    for &child in &sink[sink_id].children {
        let data = match &sink[child].data {
            SinkData::Element { name, attrs, .. } => NodeData::Element {
                name: name.local.as_ref().to_ascii_lowercase(),
                attrs: attrs
                    .iter()
                    .map(|attr| {
                        (
                            attr.name.local.as_ref().to_ascii_lowercase(),
                            attr.value.to_string(),
                        )
                    })
                    .collect(),
            },
            SinkData::Text(text) => NodeData::Text(text.clone()),
            _ => continue,
        };

        let id = dom.nodes.len();
        dom.nodes.push(Node {
            parent: Some(dom_parent),
            children: Vec::new(),
            data,
        });
        dom.nodes[dom_parent].children.push(id);
        lower(sink, child, id, dom);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags(dom: &Dom) -> Vec<&str> {
        dom.nodes.iter().filter_map(|n| n.tag()).collect()
    }

    #[test]
    fn builds_html_head_body_skeleton() {
        let dom = Dom::parse("<p>hi</p>");
        let tags = tags(&dom);
        assert!(tags.contains(&"html"));
        assert!(tags.contains(&"head"));
        assert!(tags.contains(&"body"));
        assert!(tags.contains(&"p"));
    }

    #[test]
    fn root_is_document() {
        let dom = Dom::parse("<p>hi</p>");
        assert!(matches!(dom.node(dom.root()).data, NodeData::Document));
        assert_eq!(dom.node(dom.root()).parent, None);
    }

    #[test]
    fn recovers_from_malformed_nesting() {
        // Mis-nested tags that the hand-rolled scanner could not handle.
        let dom = Dom::parse("<p>one<p>two");
        let paragraphs = dom.nodes.iter().filter(|n| n.tag() == Some("p")).count();
        assert_eq!(paragraphs, 2);
    }

    #[test]
    fn decodes_entities_in_text() {
        let dom = Dom::parse("<p>a &amp; b &lt; c</p>");
        let text: String = dom
            .nodes
            .iter()
            .filter_map(|n| match &n.data {
                NodeData::Text(t) => Some(t.as_str()),
                _ => None,
            })
            .collect();
        assert!(text.contains("a & b < c"));
    }

    #[test]
    fn exposes_attributes_and_classes() {
        let dom = Dom::parse(r#"<div class="a b" id="x">y</div>"#);
        let div = dom.nodes.iter().find(|n| n.tag() == Some("div")).unwrap();
        assert_eq!(div.attr("id"), Some("x"));
        assert_eq!(div.classes().collect::<Vec<_>>(), vec!["a", "b"]);
    }

    #[test]
    fn parent_links_are_consistent() {
        let dom = Dom::parse("<div><span>hi</span></div>");
        for (id, node) in dom.nodes.iter().enumerate() {
            for &child in &node.children {
                assert_eq!(dom.node(child).parent, Some(id));
            }
        }
    }

    #[test]
    fn drops_comments_and_doctype_keeps_structure() {
        let dom = Dom::parse("<!DOCTYPE html><!-- c --><table><tr><td>x</td></tr></table>");
        let tags = tags(&dom);
        assert!(tags.contains(&"table"));
        assert!(tags.contains(&"tr"));
        assert!(tags.contains(&"td"));
        // tbody is implied by the HTML tree builder.
        assert!(tags.contains(&"tbody"));
    }

    /// The custom arena sink must produce the same tree the reference RcDom
    /// implementation does. We compare a normalized document outline.
    #[test]
    fn matches_rcdom_reference_tree() {
        use markup5ever_rcdom::{NodeData as RcData, RcDom};

        fn our_outline(html: &str) -> Vec<String> {
            let dom = Dom::parse(html);
            let mut out = Vec::new();
            fn walk(dom: &Dom, id: NodeId, depth: usize, out: &mut Vec<String>) {
                for &child in &dom.node(id).children {
                    match &dom.node(child).data {
                        NodeData::Element { name, .. } => {
                            out.push(format!("{}e:{name}", "  ".repeat(depth)))
                        }
                        NodeData::Text(t) => {
                            out.push(format!("{}t:{}", "  ".repeat(depth), t.trim()))
                        }
                        NodeData::Document => {}
                    }
                    walk(dom, child, depth + 1, out);
                }
            }
            walk(&dom, 0, 0, &mut out);
            out
        }

        fn rc_outline(html: &str) -> Vec<String> {
            let dom = html5ever::parse_document(RcDom::default(), Default::default())
                .from_utf8()
                .read_from(&mut html.as_bytes())
                .unwrap();
            let mut out = Vec::new();
            fn walk(handle: &markup5ever_rcdom::Handle, depth: usize, out: &mut Vec<String>) {
                for child in handle.children.borrow().iter() {
                    match &child.data {
                        RcData::Element { name, .. } => out.push(format!(
                            "{}e:{}",
                            "  ".repeat(depth),
                            name.local.as_ref().to_ascii_lowercase()
                        )),
                        RcData::Text { contents } => {
                            out.push(format!("{}t:{}", "  ".repeat(depth), contents.borrow().trim()))
                        }
                        _ => continue,
                    }
                    walk(child, depth + 1, out);
                }
            }
            walk(&dom.document, 0, &mut out);
            out
        }

        for sample in [
            "<p>hi</p>",
            "<div><span>a</span>b<em>c</em></div>",
            "<p>one<p>two",
            "<table><tr><td class='x'>1</td><th>2</th></tr></table>",
            "<ul><li>a<li>b</ul>",
            "<!DOCTYPE html><html><head><title>t</title></head><body><h1>H</h1></body></html>",
        ] {
            assert_eq!(our_outline(sample), rc_outline(sample), "mismatch for {sample:?}");
        }
    }
}
