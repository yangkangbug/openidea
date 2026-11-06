# java-engine

Standalone Rust kernel for a subset of Java syntax and for name binding across
source files. It does not start a JVM, does not read class files, and does not
talk to jdtls.

The OpenIDEA editor still uses jdtls for Java. This crate is not connected to
the UI or to language-server startup. Its `Cargo.toml` does not inherit
OpenIDEA workspace settings, so the directory builds on its own. No dependency
beyond the Rust standard library is required.

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

The goal is a native Java analysis engine: syntax, binding, class path,
generics and overload resolution, then reference search. Syntax and a source-only
binder are implemented. Class files, the JDK, inheritance, and overload
selection are not.

### Syntax

- Lexer for whitespace, comments, identifiers (Unicode letters, `$`, `_`, ASCII digits),
  number forms other than hex floats, strings, text blocks, and operators.
- Parser for package and import declarations, classes, interfaces, enums,
  records, fields, methods, constructors, and the statements and expressions
  listed in `src/lib.rs`, including lambdas, method references, and
  try-with-resources (`Resource` nodes).
- Names, literals, `this`, and `super` used as expressions are nodes. Tokens stay leaves.
- Error nodes. A broken statement does not have to discard the rest of the block.
  `switch` is still an error node that skips one brace group. Patterns, modules,
  and explicit type arguments on a call (`Collections.<String>emptyList()`) are
  outside this subset.
- After an edit, a `{ ... }` block or type body is parsed again when the edit
  is strictly inside its braces and the node's new text still has balanced
  outer braces. That check reimplements IntelliJ's `checkBraceBalance`
  (see `NOTICE`). Whether `(...)` before `->` is a lambda follows
  `parseLambdaAfterParenth` from the same commit. Any other edit parses the
  whole file again. `UpdateMode::Full` means a full parse.
- `apply_edit` does not parse the file twice. `apply_edit_verified` does, and
  uses the full parse when the fragment tree differs. Tests call both and also
  compare the default path with a parse of the edited text. Those tests are
  the cases in `tests/syntax.rs`. They are not a proof that every Java program
  round-trips.
- Deleting a quote is a lexical-state change. If the new string swallows the
  closing brace, the brace check fails and the edit is a full parse. If the
  string ends at the newline and the braces still balance, the enclosing block
  is parsed again and the sibling method node is kept.

### Source binding

`Project` holds parsed files by caller-chosen path. `resolve_at` rebuilds the
index from those trees, so `Project::edit` cannot return a binding from the
previous text.

A simple name is searched in this order, which is the JLS 6.5 order restricted
to what this index stores:

1. Local, parameter, or resource in the innermost recorded scope. A resource's
   scope runs from the `try` through the try block, and not into `catch` or
   `finally`. A local in a nested block hides an outer local of the same name.
2. Field of a lexically enclosing type. `this.name` uses that field.
   `super` is `ReceiverNotBound`.
3. Member type of a lexically enclosing type. A nested type of a sibling class
   is not a candidate.
4. Single-type import. If that type is not in the project, the result is
   `Unresolved`. The search does not continue to the same file or package.
5. Top-level type in the same compilation unit.
6. Top-level type in the same package.
7. On-demand imports, plus `java.lang` only when a source file in the project
   actually declares that type. The JDK is not assumed to exist.

A qualified name (`b.Foo`, `box.value`) walks the longest indexed class prefix
and then one member. A package prefix of an indexed class is `Package`, not a
type. `Type::new` is `ConstructorRefNotBound`.

Statuses that refuse to guess:

| Status | When |
| --- | --- |
| `Resolved` | Exactly one candidate |
| `Unresolved` | No candidate, including a missing import target |
| `Ambiguous` | Two or more types or fields. The first one is not chosen |
| `OverloadNotResolved` | Two or more methods share the name. Argument count and types are not compared |
| `ReceiverNotBound` | The qualifier is a value whose declared type is not an indexed source class, or it is `super` |
| `StaticImportNotApplied` | The simple name is a static import. Those imports are recorded and not applied |
| `Package` | The name is only a package prefix |
| `ConstructorRefNotBound` | The selector is `new` after `::` |

The one-candidate rule is the same decision as `ResolveClassUtil.resolveClass`
at the commit in `NOTICE`. The scope processor is not a translation of
`ClassResolverProcessor`.

Not implemented, and not claimed:

- Class files, a classpath, or a JDK. A type that is not in `Project` stays
  unresolved, including `java.lang.String` unless that source was inserted.
- Supertypes. A method declared only on a superclass is `Unresolved` in the
  subclass. `extends` is not walked.
- Overload selection, generic inference, and capture. A generic type argument
  list can be parsed. It is not instantiated.
- Static imports, accessibility, modules, and local classes (a class inside a
  method is not a statement in this parser).
- Multi-catch. The catch parameter is bound, and its recorded type is the first
  type before `|`.
- Reference search, rename, and refactoring.
- Reuse inside the one reparsed brace node. That node is parsed again from its
  text. Green nodes outside it keep their allocations.

## IntelliJ map

Pinned tree: `840d3be11dfdeada533d05052008b48816b6cc4e`. This is not a PSI port
and not an IntelliJ-equivalent resolver.

| Piece | Status |
| --- | --- |
| Brace-balance reparse check | Reimplemented from `SyntaxBuilderUtil.checkBraceBalance` |
| Lambda lookahead | Reimplemented from `parseLambdaAfterParenth` |
| Statement recovery | Reimplemented from `StatementParser.parseStatements`: one bad token, then continue |
| One class candidate | Same decision as `ResolveClassUtil.resolveClass`: one result, otherwise none |
| Node ranges | Leading trivia stays on the parent. A literal node's slice is the literal |
| Method reference `::` | Independent parser |
| `PrattExpressionParser` as a whole | Not translated |
| `ClassResolverProcessor` and the scope walk | Not translated. Search order above is the restricted JLS order |
| PSI, stubs, stub indexes, find-usages | Not started |
| Class files, project model, compiler, inference | Not started |

## Layout

`ParsedFile::parse` builds an owned green tree (`Arc` nodes). `apply_edit`
takes UTF-8 byte offsets. `apply_utf16_edit` converts UTF-16 offsets first;
a supplementary character is two UTF-16 units, and an offset inside one is
rejected. `Project::insert`, `Project::edit`, and `Project::resolve_at` are the
binding API. The public API depends only on the standard library.

## Tests

`tests/fixtures/` was written for this crate. It is not IntelliJ `testData`.
`tests/syntax.rs` checks valid and broken files, try-with-resources, qualified
names, generics (`List<List<String>>`), sibling reuse, a cross-method edit,
UTF-16 emoji offsets, and quote deletion. Each edit is compared with
`apply_edit_verified` and with a full parse. `tests/bind.rs` checks same-named
classes, explicit and star imports, qualified names, nested members, local
shadowing, resources, overload refusal, a missing import, static imports, and
an edit that changes the next query. `tests/baseline.rs` times one fixed sample
in-process. That number is not a comparison with IDEA, and the test count is
not a coverage proof.

From the OpenIDEA workspace, with Rust 1.96:

```
cargo +1.96.0 fmt -p java-engine -- --check
cargo +1.96.0 clippy -p java-engine --all-targets --locked -- -D warnings
cargo +1.96.0 test -p java-engine --locked
```

From a copy of this directory, drop `-p java-engine`. `cargo test --locked`
uses the `Cargo.lock` in this directory.

## Later stages

1. A Rust class-file signature reader, then resolution of types that are not
   in the source set. Until that exists, external types stay unresolved.
2. Supertype lookup, then overload selection and generics inference, each with
   its own tests against the JLS. IntelliJ and jdtls are oracles to compare,
   not definitions.
3. Reference search, then one refactoring at a time.

Passing the tests in this crate does not mean those stages are done.
