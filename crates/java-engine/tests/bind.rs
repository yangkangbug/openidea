use java_engine::{ParsedFile, Project, ResolveStatus, TargetKind, TextEdit};

fn project(files: &[(&str, &str)]) -> Project {
    let mut project = Project::new();
    for (path, text) in files {
        project.insert(*path, text);
    }
    project
}

fn at(text: &str, needle: &str) -> u32 {
    text.find(needle).unwrap() as u32
}

#[test]
fn explicit_import_beats_a_same_named_class_in_another_package() {
    let use_src = "package app;\nimport b.Foo;\nclass Use { Foo value; }\n";
    let mut sources = project(&[
        ("a/Foo.java", "package a;\nclass Foo {}\n"),
        ("b/Foo.java", "package b;\nclass Foo {}\n"),
        ("app/Use.java", use_src),
    ]);
    let result = sources.resolve_at("app/Use.java", at(use_src, "Foo value"));
    assert_eq!(result.status, ResolveStatus::Resolved);
    assert_eq!(result.targets.len(), 1);
    assert_eq!(result.targets[0].owner, "b.Foo");
    assert_eq!(result.targets[0].path, "b/Foo.java");

    let renamed = sources.edit(
        "b/Foo.java",
        TextEdit {
            start_byte: at("package b;\nclass Foo {}\n", "Foo"),
            end_byte: at("package b;\nclass Foo {}\n", "Foo") + 3,
            insert: "Bar".into(),
        },
    );
    assert!(renamed.is_some());
    let after = sources.resolve_at("app/Use.java", at(use_src, "Foo value"));
    assert_eq!(after.status, ResolveStatus::Unresolved);
    assert!(after.targets.is_empty());
}

#[test]
fn same_package_hides_a_star_import_and_two_stars_are_ambiguous() {
    let use_src = "package a;\nimport b.*;\nclass Use { Foo value; }\n";
    let sources = project(&[
        ("a/Foo.java", "package a;\nclass Foo {}\n"),
        ("b/Foo.java", "package b;\nclass Foo {}\n"),
        ("a/Use.java", use_src),
    ]);
    let result = sources.resolve_at("a/Use.java", at(use_src, "Foo value"));
    assert_eq!(result.status, ResolveStatus::Resolved);
    assert_eq!(result.targets[0].owner, "a.Foo");

    let clash = "package c;\nimport a.*;\nimport b.*;\nclass Use { Foo value; }\n";
    let sources = project(&[
        ("a/Foo.java", "package a;\nclass Foo {}\n"),
        ("b/Foo.java", "package b;\nclass Foo {}\n"),
        ("c/Use.java", clash),
    ]);
    let result = sources.resolve_at("c/Use.java", at(clash, "Foo value"));
    assert_eq!(result.status, ResolveStatus::Ambiguous);
    assert_eq!(result.targets.len(), 2);
}

#[test]
fn a_missing_single_import_does_not_fall_through() {
    let use_src = "package a;\nimport missing.Foo;\nclass Foo {}\nclass Use { Foo value; }\n";
    let sources = project(&[("a/Use.java", use_src)]);
    let result = sources.resolve_at("a/Use.java", at(use_src, "Foo value"));
    assert_eq!(result.status, ResolveStatus::Unresolved);
    assert!(result.targets.is_empty());
}

#[test]
fn locals_shadow_fields_and_this_still_sees_the_field() {
    let src = "class C {\n  int x;\n  void m() {\n    int y = x;\n    int x = 2;\n    int z = x;\n    int w = this.x;\n  }\n}\n";
    let sources = project(&[("C.java", src)]);
    let before = sources.resolve_at("C.java", at(src, "y = x") + 4);
    assert_eq!(before.status, ResolveStatus::Resolved);
    assert_eq!(before.targets[0].kind, TargetKind::Field);
    let after = sources.resolve_at("C.java", at(src, "z = x") + 4);
    assert_eq!(after.status, ResolveStatus::Resolved);
    assert_eq!(after.targets[0].kind, TargetKind::Local);
    let via_this = sources.resolve_at("C.java", at(src, "this.x") + 5);
    assert_eq!(via_this.status, ResolveStatus::Resolved);
    assert_eq!(via_this.targets[0].kind, TargetKind::Field);
}

#[test]
fn overloads_stay_unpicked_and_a_source_superclass_method_is_visible() {
    let src = "class A { void m(int x) {} void m(String s) {} void inherited() {} void f() { m(1); } }\nclass B extends A { void g() { inherited(); } }\n";
    let sources = project(&[("A.java", src)]);
    let call = sources.resolve_at("A.java", at(src, "m(1)"));
    assert_eq!(call.status, ResolveStatus::OverloadNotResolved);
    assert_eq!(call.targets.len(), 2);
    assert!(call.targets.iter().all(|target| target.kind == TargetKind::Method));
    let inherited = sources.resolve_at("A.java", at(src, "inherited();"));
    assert_eq!(inherited.status, ResolveStatus::Resolved);
    assert_eq!(inherited.targets.len(), 1);
    assert_eq!(inherited.targets[0].owner, "A");
    assert_eq!(inherited.targets[0].name, "inherited");
}

#[test]
fn a_same_source_signature_hides_the_superclass_method() {
    let src = "class A { void m() {} }\nclass B extends A { void m() { m(); } }\n";
    let sources = project(&[("A.java", src)]);
    let call = sources.resolve_at("A.java", at(src, "m();"));
    assert_eq!(call.status, ResolveStatus::Resolved);
    assert_eq!(call.targets.len(), 1);
    assert_eq!(call.targets[0].owner, "B");
}

#[test]
fn a_call_does_not_fall_through_to_a_class_and_a_for_variable_ends_with_the_loop() {
    let call = "class Foo {}\nclass C { void m() { Foo(); } }\n";
    let sources = project(&[("C.java", call)]);
    assert_eq!(sources.resolve_at("C.java", at(call, "Foo()")).status, ResolveStatus::Unresolved);

    let src = "class C { void m() { for (int i = 0; i < 1; i = i + 1) { int k = i; } int z = i; } }\n";
    let sources = project(&[("C.java", src)]);
    let inside = sources.resolve_at("C.java", at(src, "k = i") + 4);
    assert_eq!(inside.status, ResolveStatus::Resolved);
    assert_eq!(inside.targets[0].kind, TargetKind::Local);
    assert_eq!(sources.resolve_at("C.java", at(src, "z = i") + 4).status, ResolveStatus::Unresolved);
}

#[test]
fn contradictory_access_flags_are_not_treated_as_public() {
    let src = "class C { public private int x; void m() { int y = x; } }\n";
    let sources = project(&[("C.java", src)]);
    let result = sources.resolve_at("C.java", at(src, "y = x") + 4);
    assert_eq!(result.status, ResolveStatus::NotDetermined);
}

#[test]
fn qualified_name_field_and_resource_are_distinct() {
    let src = "package app;\nclass Box { int value; }\nclass Use {\n  int value;\n  void m() {\n    Box box = null;\n    int n = box.value;\n    try (Box in = null) { int k = in.value; }\n  }\n}\n";
    let sources = project(&[("Use.java", src)]);
    let field = sources.resolve_at("Use.java", at(src, "box.value") + "box.".len() as u32);
    assert_eq!(field.status, ResolveStatus::Resolved, "{field:?}");
    assert_eq!(field.targets[0].owner, "app.Box");
    assert_eq!(field.targets[0].kind, TargetKind::Field);
    let resource = sources.resolve_at("Use.java", at(src, "in.value"));
    assert_eq!(resource.status, ResolveStatus::Resolved);
    assert_eq!(resource.targets[0].kind, TargetKind::Resource);
}

#[test]
fn a_quote_edit_changes_tokens_and_still_matches_a_full_parse() {
    let source = "class C { void a() { String s = \"{\"; int n = 1; } void b() { int m = 2; } }";
    let file = ParsedFile::parse(source);
    let quote = source.find("\"{\"").unwrap() as u32;
    let fast = file.apply_edit(TextEdit { start_byte: quote, end_byte: quote + 1, insert: String::new() });
    let checked = file.apply_edit_verified(TextEdit { start_byte: quote, end_byte: quote + 1, insert: String::new() });
    assert_eq!(fast.mode, checked.mode);
    assert_eq!(fast.file.root().as_ref(), ParsedFile::parse(fast.file.text()).root().as_ref());
    assert!(fast.file.tokens_cover_text());
}

#[test]
fn a_nested_member_hides_a_sibling_member() {
    let src = "class Outer {\n  class Foo {}\n  void m() { Foo value; }\n}\nclass Other { class Foo {} }\n";
    let sources = project(&[("Outer.java", src)]);
    let result = sources.resolve_at("Outer.java", at(src, "Foo value"));
    assert_eq!(result.status, ResolveStatus::Resolved);
    assert_eq!(result.targets[0].owner, "Outer.Foo");
}

#[test]
fn a_qualified_name_picks_that_package() {
    let src = "package app;\nclass Use { b.Foo value; }\n";
    let sources = project(&[
        ("a/Foo.java", "package a;\nclass Foo {}\n"),
        ("b/Foo.java", "package b;\nclass Foo {}\n"),
        ("app/Use.java", src),
    ]);
    let result = sources.resolve_at("app/Use.java", at(src, "b.Foo") + "b.".len() as u32);
    assert_eq!(result.status, ResolveStatus::Resolved);
    assert_eq!(result.targets[0].owner, "b.Foo");
    assert_eq!(result.targets[0].path, "b/Foo.java");
}

#[test]
fn two_classes_in_one_package_stay_ambiguous() {
    let src = "package a;\nclass Use { Foo value; }\n";
    let sources = project(&[
        ("a/Foo.java", "package a;\nclass Foo {}\n"),
        ("a/Also.java", "package a;\nclass Foo {}\n"),
        ("a/Use.java", src),
    ]);
    let result = sources.resolve_at("a/Use.java", at(src, "Foo value"));
    assert_eq!(result.status, ResolveStatus::Ambiguous);
    assert_eq!(result.targets.len(), 2);
}

#[test]
fn java_lang_is_used_only_when_that_source_is_indexed() {
    let src = "class C { String value; }\n";
    let missing = project(&[("C.java", src)]);
    assert_eq!(missing.resolve_at("C.java", at(src, "String value")).status, ResolveStatus::Unresolved);
    let present = project(&[("C.java", src), ("java/lang/String.java", "package java.lang;\nclass String {}\n")]);
    let result = present.resolve_at("C.java", at(src, "String value"));
    assert_eq!(result.status, ResolveStatus::Resolved);
    assert_eq!(result.targets[0].owner, "java.lang.String");
}

#[test]
fn an_inner_block_local_hides_the_outer_one() {
    let src = "class C { void m() { int x = 1; { int x = 2; int y = x; } int z = x; } }\n";
    let sources = project(&[("C.java", src)]);
    let inner = at(src, "int x = 2") + "int ".len() as u32;
    let outer = at(src, "int x = 1") + "int ".len() as u32;
    let use_inner = sources.resolve_at("C.java", at(src, "y = x") + 4);
    let use_outer = sources.resolve_at("C.java", at(src, "z = x") + 4);
    assert_eq!(use_inner.targets[0].start_byte, inner);
    assert_eq!(use_outer.targets[0].start_byte, outer);
    assert_eq!(use_inner.targets[0].kind, TargetKind::Local);
}

#[test]
fn a_resource_stops_at_the_try_block() {
    let src = "class C {\n  int in;\n  void m() {\n    try (Box in = null) { int k = in; }\n    catch (Exception in) { int n = in; }\n  }\n}\nclass Box {}\n";
    let sources = project(&[("C.java", src)]);
    let inside = sources.resolve_at("C.java", at(src, "k = in") + 4);
    assert_eq!(inside.status, ResolveStatus::Resolved);
    assert_eq!(inside.targets[0].kind, TargetKind::Resource);
    let handler = sources.resolve_at("C.java", at(src, "n = in") + 4);
    assert_eq!(handler.status, ResolveStatus::Resolved);
    assert_eq!(handler.targets[0].kind, TargetKind::Parameter);
}

#[test]
fn editing_a_declaration_changes_the_next_query() {
    let src = "class C { int x; void m() { int y = x; } }\n";
    let mut sources = project(&[("C.java", src)]);
    let use_at = at(src, "y = x") + 4;
    assert_eq!(sources.resolve_at("C.java", use_at).targets[0].kind, TargetKind::Field);
    let inserted = sources.edit(
        "C.java",
        TextEdit { start_byte: at(src, "int y"), end_byte: at(src, "int y"), insert: "int x = 1; ".into() },
    );
    assert!(matches!(inserted.unwrap().mode, java_engine::UpdateMode::Incremental { .. }));
    let after = sources.resolve_at("C.java", use_at + "int x = 1; ".len() as u32);
    assert_eq!(after.status, ResolveStatus::Resolved);
    assert_eq!(after.targets[0].kind, TargetKind::Local);
}

#[test]
fn static_import_is_reported_instead_of_guessed() {
    let src = "import static java.lang.Math.max;\nclass C { void m() { max(1, 2); } }\n";
    let sources = project(&[("C.java", src)]);
    let result = sources.resolve_at("C.java", at(src, "max(1"));
    assert_eq!(result.status, ResolveStatus::StaticImportNotApplied);
    assert!(result.targets.is_empty());
}
