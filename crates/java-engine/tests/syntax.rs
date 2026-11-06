use java_engine::{ParsedFile, SyntaxKind, TextEdit, UpdateMode, Utf16Edit};
use std::sync::Arc;

fn parse(text: &str) -> ParsedFile {
    ParsedFile::parse(text)
}

fn child(node: &java_engine::GreenNode, kind: SyntaxKind) -> &Arc<java_engine::GreenNode> {
    node.child_nodes().find(|child| child.kind() == kind).unwrap_or_else(|| panic!("missing {kind:?}"))
}

fn methods(file: &ParsedFile) -> Vec<&Arc<java_engine::GreenNode>> {
    let class = child(file.root(), SyntaxKind::Class);
    let body = child(class, SyntaxKind::ClassBody);
    body.child_nodes().filter(|node| node.kind() == SyntaxKind::Method).collect()
}

#[test]
fn valid_fixture_parses_without_errors() {
    let text = include_str!("fixtures/valid.java");
    let file = parse(text);
    assert!(file.errors().is_empty(), "{}\n{:#?}", file.dump(), file.errors());
    assert_eq!(file.dump(), parse(text).dump());
    assert!(file.dump().contains("Class"));
    assert!(file.dump().contains("你好"));
}

#[test]
fn broken_statement_keeps_the_following_declaration() {
    let file = parse(include_str!("fixtures/broken.java"));
    assert!(file.errors().iter().any(|error| error.message.contains("expected")));
    assert_eq!(methods(&file).len(), 2, "{}", file.dump());
    assert!(file.dump().contains("Ident[y]"), "{}", file.dump());
    assert!(file.dump().contains("Ident[z]"), "{}", file.dump());
}

#[test]
fn generics_close_with_a_split_shift_token() {
    let file = parse("class C { List<List<String>> xs; }");
    assert!(file.errors().is_empty(), "{}\n{:#?}", file.dump(), file.errors());
    assert!(file.dump().contains("TypeArgList"), "{}", file.dump());
}

#[test]
fn cast_binds_looser_than_field_access_and_paren_stays_a_value() {
    let cast = parse("class C { int m(String a) { return (int) a.f; } }");
    assert!(cast.errors().is_empty(), "{}\n{:#?}", cast.dump(), cast.errors());
    assert!(cast.dump().contains("CastExpr"), "{}", cast.dump());
    let paren = parse("class C { int m(int a) { return (a).f; } }");
    assert!(paren.dump().contains("ParenExpr"), "{}", paren.dump());
    assert!(!paren.dump().contains("CastExpr"));
}

#[test]
fn edit_inside_a_method_reuses_the_sibling_and_matches_a_full_parse() {
    let source = "class C {\n  void a() { int n = 1; }\n  void b() { int m = 2; }\n}\n";
    let file = parse(source);
    let sibling = Arc::clone(methods(&file)[1]);
    let n = source.find('1').unwrap() as u32;
    let edited = file.apply_edit(TextEdit { start_byte: n, end_byte: n + 1, insert: "3".into() });
    assert_eq!(edited.mode, UpdateMode::Incremental { kind: SyntaxKind::Block });
    assert_eq!(edited.file.dump(), parse(edited.file.text()).dump());
    assert!(Arc::ptr_eq(&sibling, methods(&edited.file)[1]));
}

#[test]
fn edit_on_a_brace_parses_the_whole_file() {
    let source = "class C { void a() { int n = 1; } void b() { int m = 2; } }";
    let file = parse(source);
    let brace = source.find("{ int n").unwrap() as u32;
    let edited = file.apply_edit(TextEdit { start_byte: brace, end_byte: brace + 1, insert: "(".into() });
    assert_eq!(edited.mode, UpdateMode::Full);
    assert_eq!(edited.file.dump(), parse(edited.file.text()).dump());
    assert!(!edited.file.errors().is_empty());
}

#[test]
fn consecutive_edits_stay_equal_to_a_full_parse() {
    let original = parse("class C { void a() { int n = 1; } void b() { int m = 2; } }");
    let first = original.text().find('1').unwrap() as u32;
    let edited = original.apply_edit(TextEdit { start_byte: first, end_byte: first + 1, insert: "4".into() });
    assert!(matches!(edited.mode, UpdateMode::Incremental { .. }));
    assert_eq!(edited.file.dump(), parse(edited.file.text()).dump());
    let second = edited.file.text().find("int m").unwrap() as u32;
    let edited = edited.file.apply_edit(TextEdit { start_byte: second, end_byte: second + 3, insert: "long".into() });
    assert_eq!(edited.mode, UpdateMode::Incremental { kind: SyntaxKind::Block });
    assert_eq!(edited.file.dump(), parse(edited.file.text()).dump());
    let brace = edited.file.text().find('{').unwrap() as u32;
    let edited = edited.file.apply_edit(TextEdit { start_byte: brace, end_byte: brace + 1, insert: "(".into() });
    assert_eq!(edited.mode, UpdateMode::Full);
    assert_eq!(edited.file.dump(), parse(edited.file.text()).dump());
}

#[test]
fn utf16_emoji_edit_matches_a_full_parse() {
    let source = "class 类 { void m() { String s = \"😀\"; } }";
    let file = parse(source);
    let emoji = file.text().find('😀').unwrap() as u32;
    let utf16 = file.byte_to_utf16(emoji).unwrap();
    assert_eq!(file.byte_to_utf16(emoji + "😀".len() as u32).unwrap() - utf16, 2);
    let edited =
        file.apply_utf16_edit(Utf16Edit { start_utf16: utf16, end_utf16: utf16 + 2, insert: "a".into() }).unwrap();
    assert_eq!(edited.file.text(), "class 类 { void m() { String s = \"a\"; } }");
    assert_eq!(edited.file.dump(), parse(edited.file.text()).dump());
    assert!(matches!(edited.mode, UpdateMode::Incremental { .. }));
    assert!(
        file.apply_utf16_edit(Utf16Edit { start_utf16: utf16 + 1, end_utf16: utf16 + 2, insert: "x".into() }).is_err()
    );
}

#[test]
fn switch_is_skipped_and_the_next_statement_remains() {
    let file = parse("class C { void m() { switch (x) { case 1: break; } int y = 1; } }");
    assert!(file.errors().iter().any(|error| error.message.contains("switch")));
    assert!(file.dump().contains("Ident[y]"), "{}", file.dump());
}

#[test]
fn empty_and_comment_only_files_parse() {
    assert!(parse("").errors().is_empty());
    assert!(parse("/* { */\n// }\n").errors().is_empty());
}
