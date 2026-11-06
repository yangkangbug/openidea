# java-engine

Standalone Rust kernel for a subset of Java syntax, class-file signatures, and
name binding. It does not start a JVM and does not talk to jdtls.

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
generics and overload resolution, then reference search. Syntax, class-file
signatures, and name binding across source files and those class files are
implemented. A JDK is not included. Overload selection and generic instantiation
are not.

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

### Names, class files, and supertypes

`Project` holds parsed files and class files by caller-chosen path.
`resolve_at` rebuilds the index from those inputs on every call. Nothing is
cached between queries, so `edit`, `add_class_bytes`, `add_class_directory`,
and `remove_class` are visible to the next query. This rebuild is not a
performance claim.

`add_class_bytes` parses one class file. `add_class_directory` reads `*.class`
files under a directory and does not follow symlinks. A truncated file, a bad
magic, a bad constant pool, or a major version outside 45..=71 (or a minor
other than 0 or 65535 when the major is at least 56) is an error and is not
inserted. JARs are not opened. Maven and Gradle are not read. The bytes have
to be handed to the project.

Class files are parsed by `cafebabe` 0.9.0 (see `NOTICE`). The reader keeps
the binary name, the superclass, interfaces, field and method descriptors, and
the raw `Signature` attribute. `$` in a binary name is treated as javac's
nesting separator. Constructors (`<init>`) are parsed and are not selected.
`<clinit>` is dropped. Generic signatures are stored and not instantiated.

A simple name is searched in this order, which is the JLS 6.5 order restricted
to what this index stores:

1. Local, parameter, or resource in the innermost recorded scope. A resource's
   scope runs from the `try` through the try block, and not into `catch` or
   `finally`. A local in a nested block hides an outer local of the same name.
2. Field of a lexically enclosing type, then the same name on `extends` and
   `implements`. A field on the nearer type hides one farther away. `this.name`
   uses that search. `super` is `ReceiverNotBound`.
3. Member type declared in a lexically enclosing type. Inherited member types
   are not searched. A nested type of a sibling class is not a candidate.
4. Single-type import. If that type is not in the project, the result is
   `Unresolved`. The search does not continue.
5. Top-level type in the same package, which includes the current compilation
   unit and any class file with that package and simple name. Two definitions
   of that name are `Ambiguous`.
6. On-demand imports, plus `java.lang` only when that type was actually added
   as source or as a class file. The JDK is not assumed to exist.

A call does not fall through to a class of the same name. `extends` and
`implements` are resolved in the compilation unit that wrote them, so `Lib` in
package `lib` finds `lib.Lib`. A class or interface with no written superclass
records `java.lang.Object` as that name; `enum` records `java.lang.Enum` and
`record` records `java.lang.Record`. Members of those types appear only when
the type itself was added.

Access of a field or method:

- `public` is visible.
- `private` is visible only inside the declaring class. It is not inherited.
- no modifier is visible only in the same package.
- `protected` is visible in the same package, inside the receiver type, or
  from a use site whose enclosing type has the declaring type as a supertype.
  JLS 6.6.2's extra rule about the qualifying type is not implemented.
- more than one of `public`, `protected`, and `private` is `NotDetermined`.

`static` and instance are not distinguished.

A qualified name (`b.Foo`, `box.value`) walks the longest indexed class prefix
and then one member. A package prefix of an indexed class is `Package`, not a
type. `Type::new` is `ConstructorRefNotBound`.

Statuses that refuse to guess:

| Status | When |
| --- | --- |
| `Resolved` | Exactly one candidate |
| `Unresolved` | No candidate, including a missing import target |
| `Ambiguous` | Two or more types or fields. The first one is not chosen |
| `OverloadNotResolved` | Two or more methods remain after a same-descriptor method nearer in the hierarchy hides a farther one. Descriptors are the JVM descriptor for a class file, or the source parameter list as written. The call's argument types are not compared, and the first method is not chosen |
| `SignatureNotCompared` | The name exists as both a source declaration and a class-file member, or a parameter list could not be read. Override and overload are both possible |
| `Cycle` | A supertype path visits a type that is already on that path |
| `NotDetermined` | Every candidate has contradictory access flags |
| `ReceiverNotBound` | The qualifier is a value whose declared type is not an indexed class, or it is `super` |
| `StaticImportNotApplied` | The simple name is a static import. Those imports are recorded and not applied |
| `Package` | The name is only a package prefix |
| `ConstructorRefNotBound` | The selector is `new` after `::` |

The one-candidate rule is the same decision as `ResolveClassUtil.resolveClass`
at the commit in `NOTICE`. The scope processor is not a translation of
`ClassResolverProcessor`.

Not implemented, and not claimed:

- A JDK or any classpath the caller did not add. `java.lang.String` stays
  unresolved until that source or class file is inserted.
- Choosing one overload from several. A call `m(1)` with both `m()` and `m(int)`
  indexed is `OverloadNotResolved`.
- Treating a source method and a class-file method of the same name as an
  override. That is `SignatureNotCompared`.
- Instantiating a `Signature` attribute or a source type argument.
- Static imports, modules, and local classes (a class inside a method is not a
  statement in this parser).
- The full protected-access rule, and any distinction between static and instance.
- Multi-catch beyond the first type before `|`.
- JARs, Maven, Gradle, reference search, rename, and refactoring.
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
| Compiled class reading | Not ported. `StubBuildingVisitor` extends ASM `ClassVisitor` and builds stubs; `ClsClassImpl` then implements `PsiClass`. This crate parses with `cafebabe` and puts the signature in the same index as a source type |
| Supertype member search | Independent walk of the indexed `extends` / `implements` links. Not `PsiClass.getSuperTypes` |
| PSI, stubs, stub indexes, find-usages | Not started |
| Compiler, inference, JAR indexes | Not started |

## Layout

`ParsedFile::parse` builds an owned green tree (`Arc` nodes). `apply_edit`
takes UTF-8 byte offsets. `apply_utf16_edit` converts UTF-16 offsets first;
a supplementary character is two UTF-16 units, and an offset inside one is
rejected. `Project::insert`, `Project::edit`, `Project::add_class_bytes`,
`Project::add_class_directory`, `Project::remove_class`, and
`Project::resolve_at` are the binding API. Dependencies are `cafebabe` 0.9.0
(0BSD) and, through it, `bitflags` 1.3.2 and `cesu8` 1.1.0 (both MIT OR
Apache-2.0). No dependency runs a JVM.

## Tests

`tests/fixtures/` was written for this crate. It is not IntelliJ `testData`.
`tests/syntax.rs` checks valid and broken files, try-with-resources, qualified
names, generics (`List<List<String>>`), sibling reuse, a cross-method edit,
UTF-16 emoji offsets, and quote deletion. Each edit is compared with
`apply_edit_verified` and with a full parse. `tests/bind.rs` checks same-named
classes, explicit and star imports, qualified names, nested members, local
shadowing, a `for` variable's scope, resources, overload refusal, a source
method hiding a superclass method with the same parameter list, a call that
does not bind to a class, a missing import, static imports, contradictory
access, and an edit that changes the next query. `tests/classfile.rs` checks
`tests/fixtures/classes/Lib.class` (JVMS §4, major 61, minor 0, assembled
rather than compiled by javac; `assemble_lib` must reproduce those bytes),
truncated input, a bad magic, a bad constant-pool tag, an unsupported version,
public versus private and package access, add/remove/replace of a class file,
a directory of `.class` files, a source type and a class file with the same
name, two interface fields, a supertype cycle, and `java.lang.Object` only
after that class file is added. `tests/baseline.rs` times one fixed sample
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

1. Overload selection, first by arity and then by types, including a comparison
   of a source parameter list with a class-file descriptor once erasure is
   known. Until then a mixed pair stays `SignatureNotCompared`.
2. Instantiating `Signature` attributes. They are stored and unused.
3. Reference search, then one refactoring at a time.

Passing the tests in this crate does not mean those stages are done.
