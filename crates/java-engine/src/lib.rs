//! Rust Java syntax and source-name kernel.
//!
//! This crate parses a subset of Java source, reparses a brace-balanced block
//! after an edit, and binds names across the source files handed to
//! [`Project`]. It does not read class files or run on a JVM. It is not wired
//! to jdtls or the editor UI.
//!
//! # What the parser accepts
//!
//! Compilation units with `package`, `import` / `import static`, and
//! `class` / `interface` / `enum` / `record` declarations. Members include
//! fields, methods, constructors, initializers, and nested types. Statements
//! include blocks, `if` / `while` / `do` / `for` / foreach, `return`, `throw`,
//! `break`, `continue`, `assert`, `try` / `catch` / `finally`, local variables,
//! and expression statements. Types include primitives, names, type arguments,
//! wildcards, and arrays. Expressions include assignment, `?:`, the JLS
//! binary operators, `instanceof`, unary and postfix operators, calls, field
//! access, `new`, casts, array initializers, lambdas, and method references.
//! Names, literals, `this`, and `super` are expression nodes, not bare tokens.
//!
//! # What it does not do
//!
//! `switch`, patterns, modules, and explicit type arguments on a call
//! (`Collections.<String>emptyList()`) are outside this subset. `switch` is
//! skipped as an error node. Try-with-resources declares the resource names.
//! [`Project`] binds simple and qualified names across source files. It does
//! not read class files, walk supertypes, or choose among overloads.

mod bind;
mod green;
mod kind;
mod lex;
mod parse;
mod reparse;

pub use bind::{Project, ResolveResult, ResolveStatus, Target, TargetKind};

pub use green::{GreenElement, GreenNode, GreenToken, SyntaxError};
pub use kind::SyntaxKind;
pub use lex::{Token, lex};
pub use reparse::balanced_brace_block;

use std::sync::Arc;

use green::utf16_to_byte;
use parse::{parse_file, parse_fragment};
use reparse::reparse_root;

/// A parsed buffer. Edits take UTF-8 byte offsets unless noted.
#[derive(Clone, Debug)]
pub struct ParsedFile {
    text: String,
    root: Arc<GreenNode>,
}

impl ParsedFile {
    pub fn parse(text: &str) -> Self {
        Self { text: text.to_string(), root: parse_file(text) }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn root(&self) -> &Arc<GreenNode> {
        &self.root
    }

    pub fn errors(&self) -> Vec<SyntaxError> {
        green::collect_errors(&self.root)
    }

    pub fn dump(&self) -> String {
        green::dump(&self.root)
    }

    pub fn utf16_len(&self) -> u32 {
        self.root.utf16_len()
    }

    pub fn byte_to_utf16(&self, byte: u32) -> Option<u32> {
        green::byte_to_utf16(&self.text, byte)
    }

    pub fn utf16_to_byte(&self, utf16: u32) -> Option<u32> {
        utf16_to_byte(&self.text, utf16)
    }

    /// Byte range of every node. Offsets match `text()` only when each token's
    /// stored lexeme is the slice of `text()` at that offset.
    pub fn node_ranges(&self) -> Vec<NodeRange> {
        let mut out = Vec::new();
        collect_ranges(self.root(), 0, &mut out);
        out
    }

    /// Every token's lexeme is exactly the source slice at the offset produced
    /// by walking the tree. This is the lossless-range check.
    pub fn tokens_cover_text(&self) -> bool {
        tokens_cover(self.root(), self.text(), 0) && self.root.utf8_len() as usize == self.text.len()
    }

    /// Reparse after replacing `start_byte..end_byte` with `insert`.
    ///
    /// A block or type body is parsed again only when the edit is strictly
    /// inside its braces and the new token list still has balanced outer
    /// braces. Otherwise the whole file is parsed. This path does not parse
    /// the file a second time to compare trees. [`Self::apply_edit_verified`]
    /// does that comparison and is what the tests use.
    pub fn apply_edit(&self, edit: TextEdit) -> EditResult {
        self.edit(edit, false)
    }

    /// Like [`Self::apply_edit`], then parses the new text from scratch.
    /// If the fragment tree differs, the full parse is returned with
    /// [`UpdateMode::Full`]. Default edits do not pay for this check.
    pub fn apply_edit_verified(&self, edit: TextEdit) -> EditResult {
        self.edit(edit, true)
    }

    fn edit(&self, edit: TextEdit, verify: bool) -> EditResult {
        let (text, snapped) = apply_text(&self.text, &edit);
        if !snapped
            && let Some((root, kind)) = reparse_root(&self.root, &self.text, &text, edit.start_byte, edit.end_byte)
        {
            let file = Self { text: text.clone(), root };
            if !verify || file.root.as_ref() == Self::parse(&text).root.as_ref() {
                return EditResult { file, mode: UpdateMode::Incremental { kind } };
            }
        }
        EditResult { file: Self::parse(&text), mode: UpdateMode::Full }
    }

    pub fn apply_utf16_edit(&self, edit: Utf16Edit) -> Result<EditResult, OffsetError> {
        let start = self.utf16_to_byte(edit.start_utf16).ok_or(OffsetError::Utf16)?;
        let end = self.utf16_to_byte(edit.end_utf16).ok_or(OffsetError::Utf16)?;
        if end < start {
            return Err(OffsetError::Utf16);
        }
        Ok(self.apply_edit(TextEdit { start_byte: start, end_byte: end, insert: edit.insert }))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NodeRange {
    pub kind: SyntaxKind,
    pub start_byte: u32,
    pub end_byte: u32,
}

fn collect_ranges(node: &GreenNode, start: u32, out: &mut Vec<NodeRange>) {
    out.push(NodeRange { kind: node.kind(), start_byte: start, end_byte: start + node.utf8_len() });
    let mut offset = start;
    for child in node.children() {
        if let Some(child) = child.as_node() {
            collect_ranges(child, offset, out);
        }
        offset += child.utf8_len();
    }
}

fn tokens_cover(node: &GreenNode, text: &str, start: u32) -> bool {
    let mut offset = start;
    for child in node.children() {
        match child {
            GreenElement::Token(token) => {
                let end = offset + token.utf8_len();
                let Some(slice) = text.get(offset as usize..end as usize) else {
                    return false;
                };
                if slice != token.text() {
                    return false;
                }
            }
            GreenElement::Node(child) => {
                if !tokens_cover(child, text, offset) {
                    return false;
                }
            }
        }
        offset += child.utf8_len();
    }
    offset == start + node.utf8_len()
}

/// UTF-8 range replacement. Offsets are bytes, not UTF-16 code units.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextEdit {
    pub start_byte: u32,
    pub end_byte: u32,
    pub insert: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Utf16Edit {
    pub start_utf16: u32,
    pub end_utf16: u32,
    pub insert: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OffsetError {
    /// The UTF-16 offset is past the end or inside a supplementary character.
    Utf16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdateMode {
    /// One reparseable node was parsed again. Green nodes outside it keep
    /// their previous allocations.
    Incremental { kind: SyntaxKind },
    /// The edit was not strictly inside one balanced brace node, or the
    /// fragment did not match a full parse. The file was parsed from scratch.
    Full,
}

#[derive(Clone, Debug)]
pub struct EditResult {
    pub file: ParsedFile,
    pub mode: UpdateMode,
}

fn apply_text(text: &str, edit: &TextEdit) -> (String, bool) {
    let mut start = (edit.start_byte as usize).min(text.len());
    let mut end = (edit.end_byte as usize).min(text.len());
    if end < start {
        end = start;
    }
    let mut snapped = start != edit.start_byte as usize || end != edit.end_byte as usize;
    while start > 0 && !text.is_char_boundary(start) {
        start -= 1;
        snapped = true;
    }
    while end < text.len() && !text.is_char_boundary(end) {
        end += 1;
        snapped = true;
    }
    let mut out = String::with_capacity(text.len() - (end - start) + edit.insert.len());
    out.push_str(&text[..start]);
    out.push_str(&edit.insert);
    out.push_str(&text[end..]);
    (out, snapped)
}

/// Parse `text` as one reparseable node. Exposed for tests of the fragment entry.
pub fn parse_reparseable(kind: SyntaxKind, text: &str) -> Arc<GreenNode> {
    parse_fragment(kind, text)
}
