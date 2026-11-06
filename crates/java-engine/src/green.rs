//! Owned syntax tree. Nodes are reference-counted so an edit can reuse
//! unchanged subtrees by allocation identity.

use std::sync::Arc;

use crate::kind::SyntaxKind;

#[derive(Clone, Debug)]
pub struct GreenToken {
    kind: SyntaxKind,
    text: String,
    utf16_len: u32,
}

impl GreenToken {
    pub fn new(kind: SyntaxKind, text: String) -> Arc<Self> {
        let utf16_len = utf16_len(&text);
        Arc::new(Self { kind, text, utf16_len })
    }

    pub fn kind(&self) -> SyntaxKind {
        self.kind
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn utf8_len(&self) -> u32 {
        self.text.len() as u32
    }

    pub fn utf16_len(&self) -> u32 {
        self.utf16_len
    }
}

impl PartialEq for GreenToken {
    fn eq(&self, other: &Self) -> bool {
        self.kind == other.kind && self.text == other.text
    }
}

impl Eq for GreenToken {}

#[derive(Clone, Debug)]
pub enum GreenElement {
    Node(Arc<GreenNode>),
    Token(Arc<GreenToken>),
}

impl GreenElement {
    pub fn utf8_len(&self) -> u32 {
        match self {
            Self::Node(n) => n.utf8_len(),
            Self::Token(t) => t.utf8_len(),
        }
    }

    pub fn utf16_len(&self) -> u32 {
        match self {
            Self::Node(n) => n.utf16_len(),
            Self::Token(t) => t.utf16_len(),
        }
    }

    pub fn as_node(&self) -> Option<&Arc<GreenNode>> {
        match self {
            Self::Node(n) => Some(n),
            Self::Token(_) => None,
        }
    }

    pub fn as_token(&self) -> Option<&Arc<GreenToken>> {
        match self {
            Self::Token(t) => Some(t),
            Self::Node(_) => None,
        }
    }
}

impl PartialEq for GreenElement {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Node(a), Self::Node(b)) => a.as_ref() == b.as_ref(),
            (Self::Token(a), Self::Token(b)) => a.as_ref() == b.as_ref(),
            _ => false,
        }
    }
}

impl Eq for GreenElement {}

#[derive(Clone, Debug)]
pub struct GreenNode {
    kind: SyntaxKind,
    children: Vec<GreenElement>,
    message: Option<&'static str>,
    utf8_len: u32,
    utf16_len: u32,
}

impl GreenNode {
    pub fn from_parts(kind: SyntaxKind, children: Vec<GreenElement>, message: Option<&'static str>) -> Arc<Self> {
        let utf8_len = children.iter().map(GreenElement::utf8_len).sum();
        let utf16_len = children.iter().map(GreenElement::utf16_len).sum();
        Arc::new(Self { kind, children, message, utf8_len, utf16_len })
    }

    pub fn kind(&self) -> SyntaxKind {
        self.kind
    }

    pub fn message(&self) -> Option<&'static str> {
        self.message
    }

    pub fn children(&self) -> &[GreenElement] {
        &self.children
    }

    pub fn utf8_len(&self) -> u32 {
        self.utf8_len
    }

    pub fn utf16_len(&self) -> u32 {
        self.utf16_len
    }

    pub fn child_nodes(&self) -> impl Iterator<Item = &Arc<GreenNode>> {
        self.children.iter().filter_map(GreenElement::as_node)
    }
}

impl PartialEq for GreenNode {
    fn eq(&self, other: &Self) -> bool {
        self.kind == other.kind && self.message == other.message && self.children == other.children
    }
}

impl Eq for GreenNode {}

pub fn utf16_len(text: &str) -> u32 {
    text.chars().map(|c| c.len_utf16() as u32).sum()
}

/// Map a UTF-16 offset to a UTF-8 byte offset. `None` when `utf16` falls
/// inside a supplementary character or past the end.
pub fn utf16_to_byte(text: &str, utf16: u32) -> Option<u32> {
    let mut seen = 0u32;
    for (byte, ch) in text.char_indices() {
        if seen == utf16 {
            return Some(byte as u32);
        }
        let width = ch.len_utf16() as u32;
        if seen + width > utf16 {
            return None;
        }
        seen += width;
    }
    (seen == utf16).then_some(text.len() as u32)
}

pub fn byte_to_utf16(text: &str, byte: u32) -> Option<u32> {
    let byte = byte as usize;
    if byte > text.len() || !text.is_char_boundary(byte) {
        return None;
    }
    Some(utf16_len(&text[..byte]))
}

pub fn dump(node: &GreenNode) -> String {
    let mut out = String::new();
    dump_node(node, 0, &mut out);
    out
}

fn dump_node(node: &GreenNode, indent: usize, out: &mut String) {
    for _ in 0..indent {
        out.push_str("  ");
    }
    out.push_str(&format!("{:?}", node.kind));
    if let Some(msg) = node.message {
        out.push(' ');
        out.push_str(&escape(msg));
    }
    out.push('\n');
    for child in &node.children {
        match child {
            GreenElement::Node(n) => dump_node(n, indent + 1, out),
            GreenElement::Token(t) => {
                for _ in 0..indent + 1 {
                    out.push_str("  ");
                }
                out.push_str(&format!("{:?}[{}]\n", t.kind, escape(t.text())));
            }
        }
    }
}

fn escape(text: &str) -> String {
    let mut out = String::new();
    for ch in text.chars() {
        match ch {
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxError {
    pub message: &'static str,
    pub start_byte: u32,
    pub end_byte: u32,
    pub start_utf16: u32,
    pub end_utf16: u32,
}

pub fn collect_errors(node: &GreenNode) -> Vec<SyntaxError> {
    let mut out = Vec::new();
    walk_errors(node, 0, 0, &mut out);
    out
}

fn walk_errors(node: &GreenNode, byte: u32, utf16: u32, out: &mut Vec<SyntaxError>) {
    if node.kind == SyntaxKind::Error || node.kind == SyntaxKind::ErrorToken {
        out.push(SyntaxError {
            message: node.message.unwrap_or("invalid syntax"),
            start_byte: byte,
            end_byte: byte + node.utf8_len,
            start_utf16: utf16,
            end_utf16: utf16 + node.utf16_len,
        });
    }
    let mut b = byte;
    let mut u = utf16;
    for child in &node.children {
        match child {
            GreenElement::Node(n) => walk_errors(n, b, u, out),
            GreenElement::Token(t) if t.kind == SyntaxKind::ErrorToken => out.push(SyntaxError {
                message: "invalid token",
                start_byte: b,
                end_byte: b + t.utf8_len(),
                start_utf16: u,
                end_utf16: u + t.utf16_len(),
            }),
            _ => {}
        }
        b += child.utf8_len();
        u += child.utf16_len();
    }
}
