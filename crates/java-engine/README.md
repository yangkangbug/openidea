# java-engine

Standalone Rust syntax kernel for a subset of Java. It does not start a JVM,
does not talk to jdtls, and does not resolve names or types.

The OpenIDEA editor still uses jdtls for Java. This crate is not connected to
the UI or to language-server startup.

## Goal and what this crate actually does

The goal of the kernel is a native Java analysis engine: syntax, then later
binding, class path, generics and overload resolution, then reference search.
Only the syntax slice below is implemented.

Implemented:

- Lexer for whitespace, comments, identifiers (Unicode letters, `$`, `_`, ASCII digits),
  number forms other than hex floats, strings, text blocks, and operators.
- Parser for package and import declarations, classes, interfaces, enums,
  records, fields, methods, constructors, and the statements and expressions
  listed in `src/lib.rs`.
- Error nodes. A broken statement does not have to discard the rest of the block.
  `switch` and try-with-resources are recognized only so they can be skipped.
- After an edit, a `{ ... }` block or type body is parsed again when the edit
  is strictly inside its braces and the node's new token list still has
  balanced outer braces. That check is a reimplementation of IntelliJ's
  `SyntaxBuilderUtil.checkBraceBalance` (see `NOTICE`). Any other edit parses
  the whole file again. `UpdateMode::Full` means a full parse.

Not implemented, and not claimed:

- Symbol binding, class files, JDK resolution, generics inference, overload
  resolution, reference search, rename, or refactoring.
- Lambdas, method references, `switch`, patterns, modules, and explicit type
  arguments on a call.
- Incremental reuse of anything other than green nodes outside the one
  reparsed brace node. Reparsing that node is not a token-diff inside it.

## Layout

`ParsedFile::parse` builds an owned green tree (`Arc` nodes). `apply_edit`
takes UTF-8 byte offsets. `apply_utf16_edit` converts UTF-16 offsets first;
a supplementary character is two UTF-16 units, and an offset inside one is
rejected. The public API depends only on the standard library.

## Tests

`tests/fixtures/` was written for this crate. It is not IntelliJ `testData`.

## Later stages

1. Binding for packages, imports, and simple member lookup, with a class-file
   signature reader written in Rust.
2. Generics, overload resolution, and inference, as their own tests against
   the JLS. IntelliJ and jdtls are oracles to compare, not definitions.
3. Declaration index and reference search, then one refactoring at a time.

Those stages are not started here.
