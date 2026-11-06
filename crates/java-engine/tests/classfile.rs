//! Class-file fixtures and their participation in name binding.
//!
//! `tests/fixtures/classes/Lib.class` is a JVMS §4 class file with major 61
//! (the Java 17 class version) and minor 0. It was not produced by javac.
//! `assemble_lib` below writes the same bytes; the test checks they match.
//! `cargo test` does not start a JVM.

use java_engine::{ClassErrorKind, Project, ResolveStatus, TargetKind, TextEdit, parse_class_bytes};
use std::fs;
use std::path::PathBuf;

fn at(text: &str, needle: &str) -> u32 {
    text.find(needle).unwrap() as u32
}

fn u2(value: u16) -> [u8; 2] {
    value.to_be_bytes()
}

struct Pool {
    bytes: Vec<u8>,
    count: u16,
}

impl Pool {
    fn new() -> Self {
        Self { bytes: Vec::new(), count: 1 }
    }

    fn utf8(&mut self, text: &str) -> u16 {
        let index = self.count;
        let raw = text.as_bytes();
        self.bytes.push(1);
        self.bytes.extend_from_slice(&u2(raw.len() as u16));
        self.bytes.extend_from_slice(raw);
        self.count += 1;
        index
    }

    fn class(&mut self, name: u16) -> u16 {
        let index = self.count;
        self.bytes.push(7);
        self.bytes.extend_from_slice(&u2(name));
        self.count += 1;
        index
    }
}

struct Member {
    access: u16,
    name: String,
    descriptor: String,
}

fn class_bytes(
    binary: &str,
    super_binary: Option<&str>,
    access: u16,
    interfaces: &[&str],
    fields: &[Member],
    methods: &[Member],
    signature: Option<&str>,
) -> Vec<u8> {
    let mut pool = Pool::new();
    let this_name = pool.utf8(binary);
    let this = pool.class(this_name);
    let super_index = match super_binary {
        Some(name) => {
            let super_name = pool.utf8(name);
            pool.class(super_name)
        }
        None => 0,
    };
    let mut interface_indexes = Vec::new();
    for name in interfaces {
        let iface = pool.utf8(name);
        interface_indexes.push(pool.class(iface));
    }
    let mut member_indexes = Vec::new();
    for member in fields.iter().chain(methods.iter()) {
        let name = pool.utf8(&member.name);
        let descriptor = pool.utf8(&member.descriptor);
        member_indexes.push((name, descriptor));
    }
    let signature_name = if signature.is_some() { Some(pool.utf8("Signature")) } else { None };
    let signature_value = signature.map(|text| pool.utf8(text));

    let mut out = Vec::new();
    out.extend_from_slice(&0xCAFEBABEu32.to_be_bytes());
    out.extend_from_slice(&u2(0));
    out.extend_from_slice(&u2(61));
    out.extend_from_slice(&u2(pool.count));
    out.extend_from_slice(&pool.bytes);
    out.extend_from_slice(&u2(access));
    out.extend_from_slice(&u2(this));
    out.extend_from_slice(&u2(super_index));
    out.extend_from_slice(&u2(interface_indexes.len() as u16));
    for index in interface_indexes {
        out.extend_from_slice(&u2(index));
    }
    out.extend_from_slice(&u2(fields.len() as u16));
    for (index, field) in fields.iter().enumerate() {
        let (name, descriptor) = member_indexes[index];
        out.extend_from_slice(&u2(field.access));
        out.extend_from_slice(&u2(name));
        out.extend_from_slice(&u2(descriptor));
        out.extend_from_slice(&u2(0));
    }
    out.extend_from_slice(&u2(methods.len() as u16));
    for (index, method) in methods.iter().enumerate() {
        let (name, descriptor) = member_indexes[fields.len() + index];
        out.extend_from_slice(&u2(method.access));
        out.extend_from_slice(&u2(name));
        out.extend_from_slice(&u2(descriptor));
        out.extend_from_slice(&u2(0));
    }
    match (signature_name, signature_value) {
        (Some(name), Some(value)) => {
            out.extend_from_slice(&u2(1));
            out.extend_from_slice(&u2(name));
            out.extend_from_slice(&2u32.to_be_bytes());
            out.extend_from_slice(&u2(value));
        }
        _ => out.extend_from_slice(&u2(0)),
    }
    out
}

fn member(access: u16, name: &str, descriptor: &str) -> Member {
    Member { access, name: name.to_string(), descriptor: descriptor.to_string() }
}

fn assemble_lib() -> Vec<u8> {
    class_bytes(
        "lib/Lib",
        Some("java/lang/Object"),
        0x0021,
        &[],
        &[member(0x0001, "value", "I")],
        &[
            member(0x0001, "run", "()V"),
            member(0x0001, "m", "()V"),
            member(0x0001, "m", "(I)V"),
            member(0x0002, "hidden", "()V"),
            member(0x0000, "pack", "()V"),
        ],
        Some("<T:Ljava/lang/Object;>Ljava/lang/Object;"),
    )
}

fn project_with_lib() -> Project {
    let mut project = Project::new();
    project.add_class_bytes("lib/Lib.class", &assemble_lib()).unwrap();
    project
}

#[test]
fn the_committed_class_matches_the_assembler_and_exposes_descriptors() {
    let bytes = include_bytes!("fixtures/classes/Lib.class");
    assert_eq!(bytes, assemble_lib().as_slice());
    let class = parse_class_bytes("lib/Lib.class", bytes).unwrap();
    assert_eq!(class.major, 61);
    assert_eq!(class.minor, 0);
    assert_eq!(class.binary_name, "lib/Lib");
    assert_eq!(class.dotted_name, "lib.Lib");
    assert_eq!(class.super_class.as_deref(), Some("java.lang.Object"));
    assert_eq!(class.signature.as_deref(), Some("<T:Ljava/lang/Object;>Ljava/lang/Object;"));
    assert_eq!(class.fields.len(), 1);
    assert_eq!(class.fields[0].name, "value");
    assert_eq!(class.fields[0].descriptor, "I");
    let methods: Vec<_> =
        class.methods.iter().map(|method| (method.name.as_str(), method.descriptor.as_str())).collect();
    assert_eq!(methods, vec![("run", "()V"), ("m", "()V"), ("m", "(I)V"), ("hidden", "()V"), ("pack", "()V")]);
}

#[test]
fn truncated_magic_pool_and_version_are_errors() {
    let lib = assemble_lib();
    let truncated = parse_class_bytes("short.class", &lib[..8]).unwrap_err();
    assert_eq!(truncated.kind, ClassErrorKind::Truncated);
    let mut magic = lib.clone();
    magic[0] = 0;
    assert_eq!(parse_class_bytes("magic.class", &magic).unwrap_err().kind, ClassErrorKind::BadMagic);
    let mut tag = lib.clone();
    let pool_tag = lib.windows(5).position(|window| window == [0, 61, 0, 19, 1]).unwrap() + 4;
    tag[pool_tag] = 99;
    assert_eq!(parse_class_bytes("pool.class", &tag).unwrap_err().kind, ClassErrorKind::BadConstantPool);
    let mut version = lib.clone();
    version[6] = 0;
    version[7] = 1;
    assert_eq!(parse_class_bytes("version.class", &version).unwrap_err().kind, ClassErrorKind::UnsupportedVersion);
}

#[test]
fn a_source_type_sees_public_members_of_an_added_class_and_not_a_missing_one() {
    let src = "package app;\nclass Use extends lib.Lib {\n  void m() {\n    run();\n    int n = value;\n    hidden();\n    pack();\n    toString();\n  }\n}\n";
    let mut sources = project_with_lib();
    sources.insert("Use.java", src);
    let run = sources.resolve_at("Use.java", at(src, "run()"));
    assert_eq!(run.status, ResolveStatus::Resolved, "{run:?}");
    assert_eq!(run.targets[0].owner, "lib.Lib");
    assert_eq!(run.targets[0].name, "run");
    assert_eq!(run.targets[0].path, "lib/Lib.class");
    let field = sources.resolve_at("Use.java", at(src, "n = value") + 4);
    assert_eq!(field.status, ResolveStatus::Resolved, "{field:?}");
    assert_eq!(field.targets[0].kind, TargetKind::Field);
    assert_eq!(field.targets[0].owner, "lib.Lib");
    assert_eq!(sources.resolve_at("Use.java", at(src, "hidden()")).status, ResolveStatus::Unresolved);
    assert_eq!(sources.resolve_at("Use.java", at(src, "pack()")).status, ResolveStatus::Unresolved);
    assert_eq!(sources.resolve_at("Use.java", at(src, "toString()")).status, ResolveStatus::Unresolved);

    let same = "package lib;\nclass Use extends Lib { void m() { pack(); } }\n";
    let mut sources = project_with_lib();
    sources.insert("Use.java", same);
    let pack = sources.resolve_at("Use.java", at(same, "pack()"));
    assert_eq!(pack.status, ResolveStatus::Resolved, "{pack:?}");
    assert_eq!(pack.targets[0].name, "pack");
}

#[test]
fn adding_removing_and_replacing_a_class_changes_the_next_query() {
    let src = "package app;\nclass Use extends lib.Lib { void m() { run(); } }\n";
    let mut sources = Project::new();
    sources.insert("Use.java", src);
    assert_eq!(sources.resolve_at("Use.java", at(src, "run()")).status, ResolveStatus::Unresolved);
    sources.add_class_bytes("lib/Lib.class", &assemble_lib()).unwrap();
    assert_eq!(sources.resolve_at("Use.java", at(src, "run()")).status, ResolveStatus::Resolved);
    let without_run =
        class_bytes("lib/Lib", Some("java/lang/Object"), 0x0021, &[], &[], &[member(0x0001, "other", "()V")], None);
    sources.add_class_bytes("lib/Lib.class", &without_run).unwrap();
    assert_eq!(sources.resolve_at("Use.java", at(src, "run()")).status, ResolveStatus::Unresolved);
    sources.add_class_bytes("lib/Lib.class", &assemble_lib()).unwrap();
    assert_eq!(sources.resolve_at("Use.java", at(src, "run()")).status, ResolveStatus::Resolved);
    assert!(sources.remove_class("lib/Lib.class"));
    assert_eq!(sources.resolve_at("Use.java", at(src, "run()")).status, ResolveStatus::Unresolved);

    let edited = sources.edit(
        "Use.java",
        TextEdit { start_byte: at(src, "run()"), end_byte: at(src, "run()") + 3, insert: "ran".into() },
    );
    assert!(matches!(edited.unwrap().mode, java_engine::UpdateMode::Incremental { .. }));
    sources.add_class_bytes("lib/Lib.class", &assemble_lib()).unwrap();
    let shifted = at(src, "run()");
    let _ = shifted;
    assert_eq!(
        sources
            .resolve_at("Use.java", at("package app;\nclass Use extends lib.Lib { void m() { ran(); } }\n", "ran()"))
            .status,
        ResolveStatus::Unresolved
    );
}

#[test]
fn a_directory_of_class_files_is_registered_and_a_duplicate_name_is_ambiguous() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target").join("class-fixture");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("lib")).unwrap();
    fs::write(dir.join("lib").join("Lib.class"), assemble_lib()).unwrap();
    let mut sources = Project::new();
    let origins = sources.add_class_directory(&dir).unwrap();
    assert_eq!(origins.len(), 1);
    let src = "import lib.Lib;\nclass Use { Lib value; }\n";
    sources.insert("Use.java", src);
    let result = sources.resolve_at("Use.java", at(src, "Lib value"));
    assert_eq!(result.status, ResolveStatus::Resolved, "{result:?}");
    assert_eq!(result.targets[0].path, origins[0]);

    let both = "package lib;\nclass Lib {}\nclass Use { Lib value; }\n";
    let mut sources = project_with_lib();
    sources.insert("Lib.java", both);
    let result = sources.resolve_at("Lib.java", at(both, "Lib value"));
    assert_eq!(result.status, ResolveStatus::Ambiguous, "{result:?}");
    assert_eq!(result.targets.len(), 2);
}

#[test]
fn inherited_methods_do_not_pick_the_first_and_cycles_are_reported() {
    let src = "package app;\nclass Use extends lib.Lib { void call() { m(1); } }\n";
    let mut sources = project_with_lib();
    sources.insert("Use.java", src);
    let call = sources.resolve_at("Use.java", at(src, "m(1)"));
    assert_eq!(call.status, ResolveStatus::OverloadNotResolved, "{call:?}");
    assert_eq!(call.targets.len(), 2);
    assert!(call.targets.iter().all(|target| target.owner == "lib.Lib"));

    let hiding = "package app;\nclass Use extends lib.Lib {\n  int value;\n  void run() {}\n  void m() { int n = value; run(); }\n}\n";
    let mut sources = project_with_lib();
    sources.insert("Use.java", hiding);
    let field = sources.resolve_at("Use.java", at(hiding, "n = value") + 4);
    assert_eq!(field.status, ResolveStatus::Resolved, "{field:?}");
    assert_eq!(field.targets[0].owner, "app.Use");
    let run = sources.resolve_at("Use.java", at(hiding, "run();"));
    assert_eq!(run.status, ResolveStatus::SignatureNotCompared, "{run:?}");
    assert_eq!(run.targets.len(), 2);

    let left = class_bytes("cyc/A", Some("cyc/B"), 0x0021, &[], &[], &[member(0x0001, "fromA", "()V")], None);
    let right = class_bytes("cyc/B", Some("cyc/A"), 0x0021, &[], &[], &[member(0x0001, "fromB", "()V")], None);
    let mut sources = Project::new();
    sources.add_class_bytes("A.class", &left).unwrap();
    sources.add_class_bytes("B.class", &right).unwrap();
    let src = "class S extends cyc.A { void m() { fromB(); } }\n";
    sources.insert("S.java", src);
    assert_eq!(sources.resolve_at("S.java", at(src, "fromB()")).status, ResolveStatus::Cycle);

    let pair = "class A extends B { void m() { missing(); } }\nclass B extends A {}\n";
    let sources = {
        let mut project = Project::new();
        project.insert("A.java", pair);
        project
    };
    assert_eq!(sources.resolve_at("A.java", at(pair, "missing()")).status, ResolveStatus::Cycle);
}

#[test]
fn two_interface_fields_stay_ambiguous_and_object_appears_only_when_added() {
    let one = class_bytes("a/I1", Some("java/lang/Object"), 0x0601, &[], &[member(0x0001, "value", "I")], &[], None);
    let two = class_bytes("a/I2", Some("java/lang/Object"), 0x0601, &[], &[member(0x0001, "value", "I")], &[], None);
    let mut sources = Project::new();
    sources.add_class_bytes("I1.class", &one).unwrap();
    sources.add_class_bytes("I2.class", &two).unwrap();
    let src = "class S implements a.I1, a.I2 { void m() { int n = value; } }\n";
    sources.insert("S.java", src);
    let field = sources.resolve_at("S.java", at(src, "n = value") + 4);
    assert_eq!(field.status, ResolveStatus::Ambiguous, "{field:?}");
    assert_eq!(field.targets.len(), 2);

    let bare = "class C { void m() { toString(); } }\n";
    let mut sources = Project::new();
    sources.insert("C.java", bare);
    assert_eq!(sources.resolve_at("C.java", at(bare, "toString()")).status, ResolveStatus::Unresolved);
    let object = class_bytes(
        "java/lang/Object",
        None,
        0x0021,
        &[],
        &[],
        &[member(0x0001, "toString", "()Ljava/lang/String;")],
        None,
    );
    sources.add_class_bytes("Object.class", &object).unwrap();
    let found = sources.resolve_at("C.java", at(bare, "toString()"));
    assert_eq!(found.status, ResolveStatus::Resolved, "{found:?}");
    assert_eq!(found.targets[0].owner, "java.lang.Object");
    assert_eq!(found.targets[0].name, "toString");
}
