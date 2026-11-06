//! Rust Java syntax kernel.
//!
//! This crate parses a subset of Java source and can reparse a brace-balanced
//! block after an edit. It does not resolve symbols, read class files, or run
//! on a JVM. It is not wired to jdtls or the editor UI.
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
//! access, `new`, casts, and array initializers.
//!
//! # What it does not do
//!
//! Lambdas, method references, `switch`, patterns, modules, `var` in types
//! beyond a local name, try-with-resources, and explicit type arguments on
//! calls (`Collections.<String>emptyList()`) are outside this subset. `switch`
//! and try-with-resources are skipped as error nodes so the following input
//! can still be parsed. Nothing here is a type checker: a tree does not mean
//! the program resolves.

mod green;
mod kind;
mod lex;
mod parse;
mod reparse;

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

    /// Reparse after replacing `start_byte..end_byte` with `insert`.
    ///
    /// When the edit sits strictly inside a brace-balanced block or type body
    /// and that node's new text still has balanced outer braces, only that
    /// node is parsed again. Otherwise the whole file is parsed again.
    /// `UpdateMode::Full` is a full parse, not a partial one.
    pub fn apply_edit(&self, edit: TextEdit) -> EditResult {
        let (text, snapped) = apply_text(&self.text, &edit);
        if snapped {
            return EditResult { file: Self::parse(&text), mode: UpdateMode::Full };
        }
        if let Some((root, kind)) = reparse_root(&self.root, &self.text, &text, edit.start_byte, edit.end_byte) {
            // A fragment parse can diverge from a whole-file parse. Discard it
            // rather than report an incremental tree that is not the same syntax.
            let file = Self { text: text.clone(), root };
            if file.dump() == Self::parse(&text).dump() {
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
