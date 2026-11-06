# java-engine

Standalone Rust syntax kernel for a subset of Java. It does not start a JVM,
does not talk to jdtls, and does not resolve names or types.

The OpenIDEA editor still uses jdtls for Java. This crate is not connected to
the UI or to language-server startup. Its `Cargo.toml` does not inherit
OpenIDEA workspace settings, so the directory builds on its own.

## Extracting this directory

This copy stays in OpenIDEA until a separate repository exists. Nothing here
deletes the crate or closes the OpenIDEA pull request.

When an empty GitHub repository URL is available, the move is:

1. Copy the contents of this directory to that repository's root. The
   directory root must contain `Cargo.toml`, `LICENSE`, `NOTICE`, `README.md`,
   `rust-toolchain.toml`, `src/`, `tests/`, and `.github/workflows/ci.yml`.
2. Do not copy OpenIDEA's root `Cargo.toml`, `Cargo.lock`, or `target/`.
3. Push that tree to the new repository. The nested workflow starts only
   after the files sit at the new root. OpenIDEA does not run
   `crates/java-engine/.github/workflows/ci.yml`.
4. Leave this OpenIDEA copy in place until that repository's CI is green.
   Removing it from the OpenIDEA workspace is a later change, not part of
   the copy.

Set `repository` in `Cargo.toml` only after that URL exists. `publish` stays
false so this is not sent to crates.io.

## Goal and what this crate actually does

The goal of the kernel is a native Java analysis engine: syntax, then later
binding, class path, generics and overload resolution, then reference search.
Only the syntax slice below is implemented.

Implemented:

- Lexer for whitespace, comments, identifiers (Unicode letters, `$`, `_`, ASCII digits),
  number forms other than hex floats, strings, text blocks, and operators.
- Parser for package and import declarations, classes, interfaces, enums,
  records, fields, methods, constructors, and the statements and expressions
  listed in `src/lib.rs`, including lambdas and method references.
- Names, literals, `this`, and `super` used as expressions are nodes. A later
  binder starts from those nodes. Tokens stay leaves.
- Error nodes. A broken statement does not have to discard the rest of the block.
  `switch` and try-with-resources are recognized only so they can be skipped.
- After an edit, a `{ ... }` block or type body is parsed again when the edit
  is strictly inside its braces and the node's new text still has balanced
  outer braces. That check reimplements IntelliJ's `checkBraceBalance`
  (see `NOTICE`). Whether `(...)` before `->` is a lambda follows
  `parseLambdaAfterParenth` from the same commit. Any other edit parses the
  whole file again. `UpdateMode::Full` means a full parse.
- `apply_edit` does not parse the file twice. `apply_edit_verified` does, and
  uses the full parse when the fragment tree differs. Tests call both.

Against IntelliJ `840d3be11dfdeada533d05052008b48816b6cc4e` this is not a PSI port:

| Piece | Status |
| --- | --- |
| Brace-balance reparse check | Reimplemented from `SyntaxBuilderUtil.checkBraceBalance` |
| Lambda lookahead | Reimplemented from `parseLambdaAfterParenth` |
| Method reference `::` | Independent parser; the node is the qualifier plus `::` plus a name or `new` |
| Expression nodes for names and literals | Independent, so a binder has a node to start from |
| `PrattExpressionParser` as a whole | Not translated |
| PSI, stubs, indexes, resolve, inference | Not started |
| Class files, project model, compiler | Not started |

Not implemented, and not claimed:

- Symbol binding, class files, JDK resolution, generics inference, overload
  resolution, reference search, rename, or refactoring.
- `switch`, patterns, modules, and explicit type arguments on a call
  (`Collections.<String>emptyList()`).
- Reuse inside the one reparsed brace node. That node is parsed again from
  its text. Green nodes outside it keep their allocations.

## Layout

`ParsedFile::parse` builds an owned green tree (`Arc` nodes). `apply_edit`
takes UTF-8 byte offsets. `apply_utf16_edit` converts UTF-16 offsets first;
a supplementary character is two UTF-16 units, and an offset inside one is
rejected. The public API depends only on the standard library.

## Tests

`tests/fixtures/` was written for this crate. It is not IntelliJ `testData`.
Edit tests compare `apply_edit` with `apply_edit_verified` and with a full
parse. `tests/baseline.rs` times one fixed sample in-process. That number is
not a comparison with IDEA.

## Later stages

1. Binding for packages, imports, and simple member lookup, with a class-file
   signature reader written in Rust. The expression nodes above are the
   starting point, not a resolver.
2. Generics, overload resolution, and inference, as their own tests against
   the JLS. IntelliJ and jdtls are oracles to compare, not definitions.
3. Declaration index and reference search, then one refactoring at a time.

Those stages are not started here.
