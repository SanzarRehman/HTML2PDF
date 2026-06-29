//! A compact, arena-backed DOM.
//!
//! HTML is parsed by `html5ever` (the spec-compliant Servo tokenizer and tree
//! builder), then lowered out of `markup5ever_rcdom`'s `Rc`/`RefCell` tree into
//! a flat `Vec<Node>` whose children are referenced by index. Downstream engine
//! code only ever touches this arena, never `Rc`, which keeps the structure
//! cache-friendly, low-overhead, and `Send` so each render stays independent and
//! parallelizable (ADR 0002).
//!
//! The RcDom is a transient parse target only; it is dropped as soon as lowering
//! finishes.

use html5ever::tendril::TendrilSink;
use markup5ever_rcdom::{Handle, NodeData as RcNodeData, RcDom};

/// Index of a node within [`Dom::nodes`].
pub type NodeId = usize;

/// The whole document as a flat arena. Node `0` is always the document root.
#[derive(Debug, Clone)]
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
        let rc = html5ever::parse_document(RcDom::default(), Default::default())
            .from_utf8()
            .read_from(&mut input.as_bytes())
            // RcDom parsing is infallible for in-memory byte input.
            .expect("in-memory HTML parsing cannot fail");

        let mut dom = Dom {
            nodes: vec![Node {
                parent: None,
                children: Vec::new(),
                data: NodeData::Document,
            }],
        };
        dom.lower_children(&rc.document, 0);
        dom
    }

    fn lower_children(&mut self, handle: &Handle, parent: NodeId) {
        for child in handle.children.borrow().iter() {
            if let Some(id) = self.lower(child, parent) {
                self.lower_children(child, id);
            }
        }
    }

    fn lower(&mut self, handle: &Handle, parent: NodeId) -> Option<NodeId> {
        let data = match &handle.data {
            RcNodeData::Element { name, attrs, .. } => {
                let attrs = attrs
                    .borrow()
                    .iter()
                    .map(|attr| {
                        (
                            attr.name.local.as_ref().to_ascii_lowercase(),
                            attr.value.to_string(),
                        )
                    })
                    .collect();
                NodeData::Element {
                    name: name.local.as_ref().to_ascii_lowercase(),
                    attrs,
                }
            }
            RcNodeData::Text { contents } => NodeData::Text(contents.borrow().to_string()),
            // Doctype, comments, processing instructions, and nested document
            // markers carry nothing renderable.
            _ => return None,
        };

        let id = self.nodes.len();
        self.nodes.push(Node {
            parent: Some(parent),
            children: Vec::new(),
            data,
        });
        self.nodes[parent].children.push(id);
        Some(id)
    }

    /// The document root node id (always `0`).
    pub fn root(&self) -> NodeId {
        0
    }

    pub fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id]
    }
}

// Read accessors for the cascade/box-tree work that consumes this DOM next
// (ADR 0002 migration steps 3-6). Kept on the type now so the DOM is the single
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
        self.attr("class")
            .unwrap_or_default()
            .split_whitespace()
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
}
