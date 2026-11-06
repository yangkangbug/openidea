//! Name binding for source files and class files registered on one project.
//!
//! The search order for a simple name follows JLS 6.5: a local or parameter
//! in the innermost scope, then a field of a lexically enclosing type, then
//! a single-type import, then a top-level type in the same compilation unit,
//! then the same package, then on-demand imports. `java.lang` is an on-demand
//! import only for types that are actually in this project. A call does not
//! fall through to a type.
//!
//! IntelliJ's `ResolveClassUtil.resolveClass` (intellij-community
//! `840d3be11dfdeada533d05052008b48816b6cc4e`) returns a class only when the
//! scope walk produced exactly one candidate. This module uses that same
//! one-candidate rule. It does not copy the resolver.
//!
//! Field and method lookup walks `extends` and `implements`, including class
//! files registered on [`Project`]. A missing supertype contributes nothing.
//! A cycle is [`ResolveStatus::Cycle`]. Two members that share a name stay
//! ambiguous or [`ResolveStatus::OverloadNotResolved`]; the first is not
//! chosen. A source shape is not compared with a class-file descriptor
//! ([`ResolveStatus::SignatureNotCompared`]). The index is rebuilt on every
//! query. There is no JDK and no classpath beyond what the caller added.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::classfile::{
    Access, ClassError, ClassSignature, MemberKind, MemberSignature, parse_class_bytes, read_class_directory,
};
use crate::green::GreenNode;
use crate::kind::SyntaxKind;
use crate::{EditResult, ParsedFile, TextEdit};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResolveStatus {
    Resolved,
    Unresolved,
    Ambiguous,
    /// More than one method has this name. Argument types are not compared.
    OverloadNotResolved,
    /// The qualifier is a value whose declared type is not a source class.
    ReceiverNotBound,
    /// A static import names this identifier, and static imports are not applied.
    StaticImportNotApplied,
    /// The name is a package prefix of a source class, not a type or member.
    Package,
    /// `Type::new`. Constructors are not chosen.
    ConstructorRefNotBound,
    /// `extends` / `implements` visits a type that is already on the path.
    Cycle,
    /// The name is declared in more than one place and a source parameter
    /// list cannot be compared with a class-file descriptor, so override and
    /// overload are both possible.
    SignatureNotCompared,
    /// The only candidates have contradictory access flags.
    NotDetermined,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TargetKind {
    Class,
    Field,
    Method,
    Local,
    Parameter,
    Resource,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub path: String,
    pub kind: TargetKind,
    pub name: String,
    /// Dotted package plus nesting, such as `demo.Outer.Inner`.
    pub owner: String,
    pub start_byte: u32,
    pub end_byte: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolveResult {
    pub status: ResolveStatus,
    pub targets: Vec<Target>,
}

impl ResolveResult {
    fn one(status: ResolveStatus, target: Target) -> Self {
        Self { status, targets: vec![target] }
    }

    fn none(status: ResolveStatus) -> Self {
        Self { status, targets: Vec::new() }
    }

    fn many(status: ResolveStatus, targets: Vec<Target>) -> Self {
        Self { status, targets }
    }
}

/// Parsed sources keyed by a caller-chosen path. The index is rebuilt from
/// these trees on each query, so [`Project::edit`] drops stale bindings.
#[derive(Clone, Debug, Default)]
pub struct Project {
    files: BTreeMap<String, ParsedFile>,
    binaries: BTreeMap<String, ClassSignature>,
}

impl Project {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, path: impl Into<String>, text: &str) {
        self.files.insert(path.into(), ParsedFile::parse(text));
    }

    pub fn file(&self, path: &str) -> Option<&ParsedFile> {
        self.files.get(path)
    }

    /// Replace `path` by applying `edit` to its current text.
    pub fn edit(&mut self, path: &str, edit: TextEdit) -> Option<EditResult> {
        let parsed = self.files.get(path)?;
        let result = parsed.apply_edit(edit);
        self.files.insert(path.to_string(), result.file.clone());
        Some(result)
    }

    /// Parse `bytes` and register them under `origin`. A later call with the
    /// same origin replaces the class. The next [`Project::resolve_at`] rebuilds
    /// the index from the current map.
    pub fn add_class_bytes(&mut self, origin: impl Into<String>, bytes: &[u8]) -> Result<(), ClassError> {
        let origin = origin.into();
        let signature = parse_class_bytes(&origin, bytes)?;
        self.binaries.insert(origin, signature);
        Ok(())
    }

    /// Register every `*.class` file under `root`. One bad file fails the call
    /// and leaves previously registered classes unchanged.
    pub fn add_class_directory(&mut self, root: &Path) -> Result<Vec<String>, ClassError> {
        let parsed = read_class_directory(root)?;
        let origins = parsed.iter().map(|class| class.origin.clone()).collect();
        for class in parsed {
            self.binaries.insert(class.origin.clone(), class);
        }
        Ok(origins)
    }

    pub fn remove_class(&mut self, origin: &str) -> bool {
        self.binaries.remove(origin).is_some()
    }

    pub fn class_signature(&self, origin: &str) -> Option<&ClassSignature> {
        self.binaries.get(origin)
    }

    /// Bind the name whose range contains `byte` (half-open).
    ///
    /// The index is built from the current sources and class files on every
    /// call. Nothing is cached between queries.
    pub fn resolve_at(&self, path: &str, byte: u32) -> ResolveResult {
        let Some(parsed) = self.files.get(path) else {
            return ResolveResult::none(ResolveStatus::Unresolved);
        };
        let index = Index::build(&self.files, &self.binaries);
        index.resolve(path, parsed, byte)
    }
}

#[derive(Clone, Debug)]
struct QualName {
    parts: Vec<String>,
}

#[derive(Clone, Debug)]
struct Member {
    name: String,
    kind: TargetKind,
    start: u32,
    end: u32,
    declared: Option<QualName>,
    access: Access,
    shape: Shape,
}

#[derive(Clone, Debug)]
struct ClassRec {
    path: String,
    package: String,
    nesting: Vec<String>,
    name_start: u32,
    name_end: u32,
    range_start: u32,
    range_end: u32,
    super_parts: Option<Vec<String>>,
    interface_parts: Vec<Vec<String>>,
    binary: bool,
    fields: Vec<Member>,
    methods: Vec<Member>,
}

impl ClassRec {
    fn dotted(&self) -> String {
        let nested = self.nesting.join(".");
        if self.package.is_empty() { nested } else { format!("{}.{}", self.package, nested) }
    }

    fn simple(&self) -> &str {
        self.nesting.last().map(String::as_str).unwrap_or("")
    }

    fn target(&self) -> Target {
        Target {
            path: self.path.clone(),
            kind: TargetKind::Class,
            name: self.simple().to_string(),
            owner: self.dotted(),
            start_byte: self.name_start,
            end_byte: self.name_end,
        }
    }
}

#[derive(Clone, Debug)]
struct LocalRec {
    name: String,
    kind: TargetKind,
    start: u32,
    end: u32,
    scope_start: u32,
    scope_end: u32,
    visible_from: u32,
    declared: Option<QualName>,
    path: String,
}

#[derive(Clone, Debug)]
struct FileRec {
    path: String,
    package: String,
    singles: Vec<(String, String)>,
    stars: Vec<String>,
    static_names: Vec<String>,
    classes: Vec<ClassRec>,
    locals: Vec<LocalRec>,
}

struct Index {
    files: Vec<FileRec>,
    binaries: Vec<ClassRec>,
}

impl Index {
    fn build(sources: &BTreeMap<String, ParsedFile>, binaries: &BTreeMap<String, ClassSignature>) -> Self {
        let files = sources.iter().map(|(path, file)| index_file(path, file)).collect();
        let binaries = binaries.values().map(class_from_signature).collect();
        Self { files, binaries }
    }

    fn classes(&self) -> impl Iterator<Item = &ClassRec> {
        self.files.iter().flat_map(|file| file.classes.iter()).chain(self.binaries.iter())
    }

    fn resolve(&self, path: &str, parsed: &ParsedFile, byte: u32) -> ResolveResult {
        let Some(file) = self.files.iter().find(|file| file.path == path) else {
            return ResolveResult::none(ResolveStatus::Unresolved);
        };
        if let Some(hit) = innermost_ref(parsed.root(), 0, byte, false) {
            return self.resolve_hit(file, parsed, byte, hit);
        }
        if let Some(target) = declaration_at(file, byte) {
            return ResolveResult::one(ResolveStatus::Resolved, target);
        }
        ResolveResult::none(ResolveStatus::Unresolved)
    }

    fn resolve_hit(&self, file: &FileRec, parsed: &ParsedFile, byte: u32, hit: Hit<'_>) -> ResolveResult {
        match hit.node.kind() {
            SyntaxKind::ReferenceExpr => {
                let name = token_name(hit.node);
                self.simple_name(file, byte, &name, hit.call)
            }
            SyntaxKind::Name => self.resolve_type_name(file, parsed, hit, byte),
            SyntaxKind::FieldExpr => self.resolve_field(file, parsed, hit, byte),
            SyntaxKind::MethodRefExpr => self.resolve_method_ref(file, parsed, hit, byte),
            _ => ResolveResult::none(ResolveStatus::Unresolved),
        }
    }

    fn simple_name(&self, file: &FileRec, byte: u32, name: &str, call: bool) -> ResolveResult {
        if let Some(result) = self.locals_named(file, byte, name) {
            return result;
        }
        if call {
            let methods = self.methods_here(file, byte, name);
            if methods.status != ResolveStatus::Unresolved {
                return methods;
            }
            if file.static_names.iter().any(|imported| imported == name) {
                return ResolveResult::none(ResolveStatus::StaticImportNotApplied);
            }
            return ResolveResult::none(ResolveStatus::Unresolved);
        } else if let Some(result) = self.fields_named(file, byte, name) {
            return result;
        }
        match self.simple_type(file, name, byte) {
            TypeLookup::One(class) => ResolveResult::one(ResolveStatus::Resolved, class.target()),
            TypeLookup::Many(classes) => {
                ResolveResult::many(ResolveStatus::Ambiguous, classes.into_iter().map(|c| c.target()).collect())
            }
            TypeLookup::MissingImport => ResolveResult::none(ResolveStatus::Unresolved),
            TypeLookup::None => {
                if file.static_names.iter().any(|imported| imported == name) {
                    ResolveResult::none(ResolveStatus::StaticImportNotApplied)
                } else {
                    ResolveResult::none(ResolveStatus::Unresolved)
                }
            }
        }
    }

    fn locals_named(&self, file: &FileRec, byte: u32, name: &str) -> Option<ResolveResult> {
        let locals: Vec<&LocalRec> = file
            .locals
            .iter()
            .filter(|local| {
                local.name == name && local.scope_start <= byte && byte < local.scope_end && local.visible_from <= byte
            })
            .collect();
        if locals.is_empty() {
            return None;
        }
        let min_span = locals.iter().map(|local| local.scope_end - local.scope_start).min().unwrap();
        let targets = locals
            .into_iter()
            .filter(|local| local.scope_end - local.scope_start == min_span)
            .map(LocalRec::target)
            .collect::<Vec<_>>();
        let status = if targets.len() == 1 { ResolveStatus::Resolved } else { ResolveStatus::Ambiguous };
        Some(ResolveResult::many(status, targets))
    }

    fn fields_named(&self, file: &FileRec, byte: u32, name: &str) -> Option<ResolveResult> {
        for class in enclosing_classes(file, byte) {
            let result = self.lookup_fields(class, name, file, byte);
            if result.status != ResolveStatus::Unresolved {
                return Some(result);
            }
        }
        None
    }

    fn methods_here(&self, file: &FileRec, byte: u32, name: &str) -> ResolveResult {
        for class in enclosing_classes(file, byte) {
            let result = self.lookup_methods(class, name, file, byte);
            if result.status != ResolveStatus::Unresolved {
                return result;
            }
        }
        ResolveResult::none(ResolveStatus::Unresolved)
    }

    fn simple_type<'a>(&'a self, file: &'a FileRec, name: &str, byte: u32) -> TypeLookup<'a> {
        if let Some(member) = member_type_here(file, byte, name) {
            return TypeLookup::One(member);
        }
        if let Some((_, full)) = file.singles.iter().find(|(simple, _)| simple == name) {
            let imported = self.classes().filter(|class| class.dotted() == *full).collect::<Vec<_>>();
            let same_unit = file
                .classes
                .iter()
                .filter(|class| class.nesting.len() == 1 && class.simple() == name)
                .collect::<Vec<_>>();
            if imported.len() == 1 && same_unit.is_empty() {
                return TypeLookup::One(imported[0]);
            }
            if imported.is_empty() {
                return TypeLookup::MissingImport;
            }
            let mut all = imported;
            all.extend(same_unit);
            return if all.len() == 1 { TypeLookup::One(all[0]) } else { TypeLookup::Many(all) };
        }
        let same_package: Vec<_> = self
            .classes()
            .filter(|class| class.package == file.package && class.nesting.len() == 1 && class.simple() == name)
            .collect();
        if same_package.len() == 1 {
            return TypeLookup::One(same_package[0]);
        }
        if same_package.len() > 1 {
            return TypeLookup::Many(same_package);
        }
        let mut stars = file.stars.clone();
        if !stars.iter().any(|package| package == "java.lang") {
            stars.push("java.lang".to_string());
        }
        let demanded: Vec<_> = self
            .classes()
            .filter(|class| stars.contains(&class.package) && class.nesting.len() == 1 && class.simple() == name)
            .collect();
        match demanded.len() {
            0 => TypeLookup::None,
            1 => TypeLookup::One(demanded[0]),
            _ => TypeLookup::Many(demanded),
        }
    }

    fn resolve_type_name(&self, file: &FileRec, _parsed: &ParsedFile, hit: Hit<'_>, byte: u32) -> ResolveResult {
        let (parts, _) = name_pieces(hit.node, hit.start);
        let Some(index) = parts.iter().position(|(_, start, end)| *start <= byte && byte < *end) else {
            return ResolveResult::none(ResolveStatus::Unresolved);
        };
        let prefix = parts[..=index].iter().map(|(name, _, _)| name.clone()).collect::<Vec<_>>();
        if prefix.len() == 1 {
            return match self.simple_type(file, &prefix[0], byte) {
                TypeLookup::One(class) => ResolveResult::one(ResolveStatus::Resolved, class.target()),
                TypeLookup::Many(classes) => {
                    ResolveResult::many(ResolveStatus::Ambiguous, classes.into_iter().map(ClassRec::target).collect())
                }
                TypeLookup::None | TypeLookup::MissingImport => ResolveResult::none(ResolveStatus::Unresolved),
            };
        }
        self.qualify(file, &prefix, false, byte)
    }

    fn resolve_field(&self, file: &FileRec, _parsed: &ParsedFile, hit: Hit<'_>, byte: u32) -> ResolveResult {
        let chain = field_chain(hit.node, hit.start);
        let Some(index) = chain.iter().position(|segment| segment.start <= byte && byte < segment.end) else {
            return ResolveResult::none(ResolveStatus::Unresolved);
        };
        self.resolve_chain(file, &chain[..=index], hit.call, byte)
    }

    fn resolve_chain(&self, file: &FileRec, chain: &[Segment], call: bool, byte: u32) -> ResolveResult {
        if chain.is_empty() {
            return ResolveResult::none(ResolveStatus::Unresolved);
        }
        if chain[0].this {
            let mut current = self.member_of_enclosing(
                file,
                chain[0].start,
                &chain.get(1).map(|segment| segment.name.clone()).unwrap_or_default(),
                call && chain.len() == 2,
            );
            if chain.len() <= 2 {
                return current;
            }
            for (index, segment) in chain.iter().enumerate().skip(2) {
                current = self.member_from_base(file, &current, &segment.name, call && index + 1 == chain.len(), byte);
            }
            return current;
        }
        let mut current = self.simple_name(file, chain[0].start, &chain[0].name, false);
        if matches!(current.status, ResolveStatus::Unresolved | ResolveStatus::StaticImportNotApplied) {
            current = self.qualify(file, &[chain[0].name.clone()], false, byte);
        }
        for (index, segment) in chain.iter().enumerate().skip(1) {
            let last = index + 1 == chain.len();
            let next = self.member_from_base(file, &current, &segment.name, call && last, byte);
            if matches!(next.status, ResolveStatus::Unresolved | ResolveStatus::ReceiverNotBound) {
                let parts = chain[..=index].iter().map(|segment| segment.name.clone()).collect::<Vec<_>>();
                let qualified = self.qualify(file, &parts, call && last, byte);
                current = if qualified.status == ResolveStatus::Unresolved { next } else { qualified };
            } else {
                current = next;
            }
        }
        current
    }

    fn resolve_method_ref(&self, file: &FileRec, parsed: &ParsedFile, hit: Hit<'_>, byte: u32) -> ResolveResult {
        if token_kind_covering(hit.node, hit.start, byte) == Some(SyntaxKind::New) {
            return ResolveResult::none(ResolveStatus::ConstructorRefNotBound);
        }
        self.resolve_field(file, parsed, hit, byte)
    }

    fn member_of_enclosing(&self, file: &FileRec, byte: u32, name: &str, call: bool) -> ResolveResult {
        let Some(class) = enclosing_classes(file, byte).into_iter().next() else {
            return ResolveResult::none(ResolveStatus::Unresolved);
        };
        self.lookup_member(class, name, call, file, byte)
    }

    fn member_from_base(
        &self,
        file: &FileRec,
        base: &ResolveResult,
        name: &str,
        call: bool,
        byte: u32,
    ) -> ResolveResult {
        match base.status {
            ResolveStatus::Resolved | ResolveStatus::Ambiguous if base.targets.len() == 1 => {
                let target = &base.targets[0];
                match target.kind {
                    TargetKind::Class => {
                        let Some(class) =
                            self.classes().find(|class| class.dotted() == target.owner && class.path == target.path)
                        else {
                            return ResolveResult::none(ResolveStatus::Unresolved);
                        };
                        self.lookup_member(class, name, call, file, byte)
                    }
                    TargetKind::Local | TargetKind::Parameter | TargetKind::Resource | TargetKind::Field => {
                        let declared = declared_of(file, target);
                        let Some(qual) = declared else {
                            return ResolveResult::none(ResolveStatus::ReceiverNotBound);
                        };
                        match self.lookup_type_name(file, &qual.parts, target.start_byte) {
                            TypeLookup::One(class) => self.lookup_member(class, name, call, file, byte),
                            TypeLookup::Many(_) => ResolveResult::none(ResolveStatus::Ambiguous),
                            TypeLookup::MissingImport | TypeLookup::None => {
                                ResolveResult::none(ResolveStatus::ReceiverNotBound)
                            }
                        }
                    }
                    TargetKind::Method => ResolveResult::none(ResolveStatus::ReceiverNotBound),
                }
            }
            ResolveStatus::Package => {
                let mut parts = Vec::new();
                if let Some(owner) = base.targets.first() {
                    parts.extend(owner.owner.split('.').map(str::to_string));
                }
                parts.push(name.to_string());
                self.qualify(file, &parts, call, byte)
            }
            _ => {
                let mut parts = Vec::new();
                if let Some(first) = base.targets.first() {
                    parts.push(first.name.clone());
                }
                parts.push(name.to_string());
                if parts.len() >= 2 {
                    self.qualify(file, &parts, call, byte)
                } else {
                    ResolveResult::none(ResolveStatus::ReceiverNotBound)
                }
            }
        }
    }

    fn qualify(&self, file: &FileRec, parts: &[String], call: bool, byte: u32) -> ResolveResult {
        if parts.is_empty() {
            return ResolveResult::none(ResolveStatus::Unresolved);
        }
        for length in (1..=parts.len()).rev() {
            let found = class_by_parts(self.classes(), &parts[..length]);
            if found.is_empty() {
                continue;
            }
            if found.len() > 1 {
                return ResolveResult::many(
                    ResolveStatus::Ambiguous,
                    found.into_iter().map(ClassRec::target).collect(),
                );
            }
            let class = found[0];
            if length == parts.len() {
                return ResolveResult::one(ResolveStatus::Resolved, class.target());
            }
            if length + 1 == parts.len() {
                return self.lookup_member(class, &parts[length], call, file, byte);
            }
            let member = self.lookup_member(class, &parts[length], false, file, byte);
            if member.status != ResolveStatus::Resolved || member.targets.len() != 1 {
                return member;
            }
            let mut rest = member;
            for next in &parts[length + 1..] {
                rest = self.member_from_base(file, &rest, next, false, byte);
            }
            return rest;
        }
        if is_package_prefix(self.classes(), parts) {
            let dotted = parts.join(".");
            return ResolveResult::one(
                ResolveStatus::Package,
                Target {
                    path: String::new(),
                    kind: TargetKind::Class,
                    name: parts.last().cloned().unwrap_or_default(),
                    owner: dotted,
                    start_byte: 0,
                    end_byte: 0,
                },
            );
        }
        ResolveResult::none(ResolveStatus::Unresolved)
    }

    fn lookup_type_name<'a>(&'a self, file: &'a FileRec, parts: &[String], byte: u32) -> TypeLookup<'a> {
        if parts.len() == 1 {
            return self.simple_type(file, &parts[0], byte);
        }
        let found = class_by_parts(self.classes(), parts);
        match found.len() {
            0 => TypeLookup::None,
            1 => TypeLookup::One(found[0]),
            _ => TypeLookup::Many(found),
        }
    }

    fn lookup_member(&self, class: &ClassRec, name: &str, call: bool, site: &FileRec, byte: u32) -> ResolveResult {
        if call { self.lookup_methods(class, name, site, byte) } else { self.lookup_fields(class, name, site, byte) }
    }

    fn lookup_fields(&self, class: &ClassRec, name: &str, site: &FileRec, byte: u32) -> ResolveResult {
        let found = match self.collect(class, name, false, site, byte) {
            Ok(found) => found,
            Err(err) => return walk_error(err),
        };
        select_fields(found)
    }

    fn lookup_methods(&self, class: &ClassRec, name: &str, site: &FileRec, byte: u32) -> ResolveResult {
        let found = match self.collect(class, name, true, site, byte) {
            Ok(found) => found,
            Err(err) => return walk_error(err),
        };
        select_methods(found)
    }

    fn collect(
        &self,
        class: &ClassRec,
        name: &str,
        call: bool,
        site: &FileRec,
        byte: u32,
    ) -> Result<Vec<Candidate>, WalkError> {
        let steps = self.hierarchy(class)?;
        let mut found = Vec::new();
        for step in steps {
            let members: Vec<&Member> = if call {
                step.class.methods.iter().filter(|member| member.name == name).collect()
            } else {
                step.class.fields.iter().filter(|member| member.name == name).collect()
            };
            for member in members {
                if !self.can_see(member, step.class, class, site, byte)? {
                    continue;
                }
                found.push(Candidate {
                    target: member_target(step.class, member),
                    depth: step.depth,
                    shape: member.shape.clone(),
                    unspecified: member.access == Access::Unspecified,
                });
            }
        }
        Ok(found)
    }

    fn hierarchy<'a>(&'a self, start: &'a ClassRec) -> Result<Vec<Step<'a>>, WalkError> {
        let mut out = Vec::new();
        let mut gray = Vec::new();
        let mut black = BTreeSet::new();
        self.walk(start, 0, &mut gray, &mut black, &mut out)?;
        Ok(out)
    }

    fn walk<'a>(
        &'a self,
        class: &'a ClassRec,
        depth: u32,
        gray: &mut Vec<String>,
        black: &mut BTreeSet<String>,
        out: &mut Vec<Step<'a>>,
    ) -> Result<(), WalkError> {
        let name = class.dotted();
        if gray.iter().any(|seen| seen == &name) {
            return Err(WalkError::Cycle);
        }
        if !black.insert(name.clone()) {
            return Ok(());
        }
        gray.push(name);
        out.push(Step { class, depth });
        let mut edges = Vec::new();
        if let Some(super_parts) = &class.super_parts {
            edges.push(super_parts.clone());
        }
        edges.extend(class.interface_parts.iter().cloned());
        for edge in edges {
            match self.resolve_edge(class, &edge) {
                Named::None => {}
                Named::Many(classes) => {
                    return Err(WalkError::Ambiguous(classes.into_iter().map(ClassRec::target).collect()));
                }
                Named::One(next) => self.walk(next, depth + 1, gray, black, out)?,
            }
        }
        gray.pop();
        Ok(())
    }

    fn can_see(
        &self,
        member: &Member,
        declaring: &ClassRec,
        start: &ClassRec,
        site: &FileRec,
        byte: u32,
    ) -> Result<bool, WalkError> {
        match member.access {
            Access::Public | Access::Unspecified => Ok(true),
            Access::Package => Ok(site.package == declaring.package),
            Access::Private => Ok(declaring.dotted() == start.dotted()
                && declaring.path == start.path
                && enclosing_classes(site, byte).iter().any(|class| same_class(class, declaring))),
            Access::Protected => {
                if site.package == declaring.package
                    || enclosing_classes(site, byte).iter().any(|class| same_class(class, start))
                {
                    return Ok(true);
                }
                for class in enclosing_classes(site, byte) {
                    let steps = self.hierarchy(class)?;
                    if steps.iter().any(|step| same_class(step.class, declaring)) {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
        }
    }

    fn resolve_edge<'a>(&'a self, class: &ClassRec, parts: &[String]) -> Named<'a> {
        if class.binary {
            return self.class_named(&parts.join("."));
        }
        let Some(file) = self.files.iter().find(|file| file.path == class.path) else {
            return Named::None;
        };
        match self.lookup_type_name(file, parts, class.name_start) {
            TypeLookup::One(found) => Named::One(found),
            TypeLookup::Many(found) => Named::Many(found),
            TypeLookup::None | TypeLookup::MissingImport => Named::None,
        }
    }

    fn class_named<'a>(&'a self, dotted: &str) -> Named<'a> {
        let found: Vec<_> = self.classes().filter(|class| class.dotted() == dotted).collect();
        match found.len() {
            0 => Named::None,
            1 => Named::One(found[0]),
            _ => Named::Many(found),
        }
    }
}

enum Named<'a> {
    None,
    One(&'a ClassRec),
    Many(Vec<&'a ClassRec>),
}

fn walk_error(err: WalkError) -> ResolveResult {
    match err {
        WalkError::Cycle => ResolveResult::none(ResolveStatus::Cycle),
        WalkError::Ambiguous(targets) => ResolveResult::many(ResolveStatus::Ambiguous, targets),
    }
}

fn select_fields(found: Vec<Candidate>) -> ResolveResult {
    if found.is_empty() {
        return ResolveResult::none(ResolveStatus::Unresolved);
    }
    if found.iter().all(|candidate| candidate.unspecified) {
        return ResolveResult::many(
            ResolveStatus::NotDetermined,
            found.into_iter().map(|candidate| candidate.target).collect(),
        );
    }
    let found: Vec<_> = found.into_iter().filter(|candidate| !candidate.unspecified).collect();
    let min = found.iter().map(|candidate| candidate.depth).min().unwrap();
    let nearest: Vec<_> =
        found.into_iter().filter(|candidate| candidate.depth == min).map(|candidate| candidate.target).collect();
    match nearest.len() {
        0 => ResolveResult::none(ResolveStatus::Unresolved),
        1 => ResolveResult::many(ResolveStatus::Resolved, nearest),
        _ => ResolveResult::many(ResolveStatus::Ambiguous, nearest),
    }
}

fn select_methods(found: Vec<Candidate>) -> ResolveResult {
    if found.is_empty() {
        return ResolveResult::none(ResolveStatus::Unresolved);
    }
    if found.iter().all(|candidate| candidate.unspecified) {
        return ResolveResult::many(
            ResolveStatus::NotDetermined,
            found.into_iter().map(|candidate| candidate.target).collect(),
        );
    }
    let found: Vec<_> = found.into_iter().filter(|candidate| !candidate.unspecified).collect();
    if found.iter().any(|candidate| candidate.shape == Shape::Unknown) {
        return ResolveResult::many(
            ResolveStatus::SignatureNotCompared,
            found.into_iter().map(|candidate| candidate.target).collect(),
        );
    }
    let mixes_representations = found.iter().any(|candidate| matches!(candidate.shape, Shape::Source(_)))
        && found.iter().any(|candidate| matches!(candidate.shape, Shape::Binary(_)));
    if mixes_representations {
        return ResolveResult::many(
            ResolveStatus::SignatureNotCompared,
            found.into_iter().map(|candidate| candidate.target).collect(),
        );
    }
    let mut groups: BTreeMap<String, Vec<Candidate>> = BTreeMap::new();
    for candidate in found {
        let key = match &candidate.shape {
            Shape::Source(text) | Shape::Binary(text) => text.clone(),
            Shape::Unknown => String::new(),
        };
        groups.entry(key).or_default().push(candidate);
    }
    let mut kept = Vec::new();
    for group in groups.into_values() {
        let min = group.iter().map(|candidate| candidate.depth).min().unwrap();
        kept.extend(group.into_iter().filter(|candidate| candidate.depth == min));
    }
    let status = if kept.len() == 1 {
        ResolveStatus::Resolved
    } else if groups_one_shape(&kept) {
        ResolveStatus::Ambiguous
    } else {
        ResolveStatus::OverloadNotResolved
    };
    ResolveResult::many(status, kept.into_iter().map(|candidate| candidate.target).collect())
}

fn groups_one_shape(kept: &[Candidate]) -> bool {
    let Some(first) = kept.first() else { return true };
    kept.iter().all(|candidate| candidate.shape == first.shape)
}

enum TypeLookup<'a> {
    One(&'a ClassRec),
    Many(Vec<&'a ClassRec>),
    None,
    MissingImport,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Shape {
    Unknown,
    Source(String),
    Binary(String),
}

fn same_class(left: &ClassRec, right: &ClassRec) -> bool {
    left.path == right.path && left.dotted() == right.dotted()
}

struct Candidate {
    target: Target,
    depth: u32,
    shape: Shape,
    unspecified: bool,
}

enum WalkError {
    Cycle,
    Ambiguous(Vec<Target>),
}

struct Step<'a> {
    class: &'a ClassRec,
    depth: u32,
}

fn member_target(class: &ClassRec, member: &Member) -> Target {
    Target {
        path: class.path.clone(),
        kind: member.kind,
        name: member.name.clone(),
        owner: class.dotted(),
        start_byte: member.start,
        end_byte: member.end,
    }
}

fn declared_of(file: &FileRec, target: &Target) -> Option<QualName> {
    if let Some(local) = file.locals.iter().find(|local| local.start == target.start_byte && local.name == target.name)
    {
        return local.declared.clone();
    }
    file.classes.iter().find_map(|class| {
        class
            .fields
            .iter()
            .find(|field| field.start == target.start_byte && field.name == target.name)
            .and_then(|field| field.declared.clone())
    })
}

fn enclosing_classes(file: &FileRec, byte: u32) -> Vec<&ClassRec> {
    let mut classes: Vec<_> =
        file.classes.iter().filter(|class| class.range_start <= byte && byte < class.range_end).collect();
    classes.sort_by_key(|class| class.range_end - class.range_start);
    classes
}

fn member_type_here<'a>(file: &'a FileRec, byte: u32, name: &str) -> Option<&'a ClassRec> {
    for class in enclosing_classes(file, byte) {
        if let Some(inner) = file.classes.iter().find(|inner| {
            inner.nesting.len() == class.nesting.len() + 1
                && inner.nesting[..class.nesting.len()] == class.nesting[..]
                && inner.simple() == name
        }) {
            return Some(inner);
        }
    }
    None
}

fn class_by_parts<'a>(classes: impl Iterator<Item = &'a ClassRec>, parts: &[String]) -> Vec<&'a ClassRec> {
    let dotted = parts.join(".");
    classes.filter(|class| class.dotted() == dotted).collect()
}

fn is_package_prefix<'a>(classes: impl Iterator<Item = &'a ClassRec>, parts: &[String]) -> bool {
    let prefix = parts.join(".");
    classes.into_iter().any(|class| {
        class.package == prefix
            || class
                .package
                .split('.')
                .collect::<Vec<_>>()
                .starts_with(&parts.iter().map(String::as_str).collect::<Vec<_>>())
    })
}

struct Hit<'a> {
    node: &'a GreenNode,
    start: u32,
    call: bool,
}

fn innermost_ref(node: &GreenNode, start: u32, byte: u32, called: bool) -> Option<Hit<'_>> {
    let end = start + node.utf8_len();
    if byte < start || byte >= end {
        return None;
    }
    let mut child_at = start;
    let mut saw_node = false;
    for child in node.children() {
        if let Some(child_node) = child.as_node() {
            let child_called = node.kind() == SyntaxKind::CallExpr && !saw_node;
            saw_node = true;
            if let Some(hit) = innermost_ref(child_node, child_at, byte, child_called) {
                return Some(hit);
            }
        }
        child_at += child.utf8_len();
    }
    if matches!(
        node.kind(),
        SyntaxKind::ReferenceExpr | SyntaxKind::Name | SyntaxKind::FieldExpr | SyntaxKind::MethodRefExpr
    ) {
        Some(Hit { node, start, call: called })
    } else {
        None
    }
}

struct Segment {
    name: String,
    start: u32,
    end: u32,
    this: bool,
}

fn field_chain(node: &GreenNode, start: u32) -> Vec<Segment> {
    if node.kind() == SyntaxKind::ThisExpr {
        return vec![Segment { name: "this".into(), start, end: start + node.utf8_len(), this: true }];
    }
    if node.kind() == SyntaxKind::ReferenceExpr {
        let mut offset = start;
        for child in node.children() {
            if let Some(token) = child.as_token()
                && token.kind() == SyntaxKind::Ident
            {
                return vec![Segment {
                    name: token.text().to_string(),
                    start: offset,
                    end: offset + token.utf8_len(),
                    this: false,
                }];
            }
            offset += child.utf8_len();
        }
    }
    if matches!(node.kind(), SyntaxKind::FieldExpr | SyntaxKind::MethodRefExpr) {
        let mut offset = start;
        let mut base = None;
        let mut selector = None;
        for child in node.children() {
            if base.is_none()
                && let Some(child_node) = child.as_node()
            {
                base = Some(field_chain(child_node, offset));
            }
            if let Some(token) = child.as_token()
                && token.kind() == SyntaxKind::Ident
            {
                selector = Some(Segment {
                    name: token.text().to_string(),
                    start: offset,
                    end: offset + token.utf8_len(),
                    this: false,
                });
            }
            offset += child.utf8_len();
        }
        let mut chain = base.unwrap_or_default();
        if let Some(selector) = selector {
            chain.push(selector);
        }
        return chain;
    }
    Vec::new()
}

fn token_kind_covering(node: &GreenNode, start: u32, byte: u32) -> Option<SyntaxKind> {
    let mut offset = start;
    for child in node.children() {
        let end = offset + child.utf8_len();
        if let Some(token) = child.as_token()
            && offset <= byte
            && byte < end
        {
            return Some(token.kind());
        }
        offset = end;
    }
    None
}

fn token_name(node: &GreenNode) -> String {
    node.children()
        .iter()
        .find_map(|child| {
            child.as_token().filter(|token| token.kind() == SyntaxKind::Ident).map(|token| token.text().to_string())
        })
        .unwrap_or_default()
}

fn declaration_at(file: &FileRec, byte: u32) -> Option<Target> {
    if let Some(local) = file.locals.iter().find(|local| local.start <= byte && byte < local.end) {
        return Some(local.target());
    }
    for class in &file.classes {
        if class.name_start <= byte && byte < class.name_end {
            return Some(class.target());
        }
        for member in class.fields.iter().chain(class.methods.iter()) {
            if member.start <= byte && byte < member.end {
                return Some(member_target(class, member));
            }
        }
    }
    None
}

impl LocalRec {
    fn target(&self) -> Target {
        Target {
            path: self.path.clone(),
            kind: self.kind,
            name: self.name.clone(),
            owner: String::new(),
            start_byte: self.start,
            end_byte: self.end,
        }
    }
}

fn index_file(path: &str, parsed: &ParsedFile) -> FileRec {
    let mut file = FileRec {
        path: path.to_string(),
        package: String::new(),
        singles: Vec::new(),
        stars: Vec::new(),
        static_names: Vec::new(),
        classes: Vec::new(),
        locals: Vec::new(),
    };
    index_node(parsed.root(), 0, &mut IndexCtx { file: &mut file, class_stack: Vec::new(), scope_stack: Vec::new() });
    file
}

struct IndexCtx<'a> {
    file: &'a mut FileRec,
    class_stack: Vec<usize>,
    scope_stack: Vec<(u32, u32, ScopeKind)>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ScopeKind {
    Other,
    Method,
    Lambda,
    Catch,
    Try,
}

fn index_node(node: &GreenNode, start: u32, ctx: &mut IndexCtx<'_>) {
    match node.kind() {
        SyntaxKind::PackageDecl => {
            if let Some(name) = child_kind(node, start, SyntaxKind::Name) {
                ctx.file.package = name_strings(name.0, name.1).join(".");
            }
        }
        SyntaxKind::ImportDecl => index_import(node, start, ctx),
        SyntaxKind::Class | SyntaxKind::Interface | SyntaxKind::Enum | SyntaxKind::Record => {
            index_class(node, start, ctx);
            return;
        }
        SyntaxKind::Field if !ctx.class_stack.is_empty() => {
            let class_index = *ctx.class_stack.last().unwrap();
            index_members(node, start, ctx, class_index, TargetKind::Field);
            return;
        }
        SyntaxKind::Method | SyntaxKind::Constructor if !ctx.class_stack.is_empty() => {
            let class_index = *ctx.class_stack.last().unwrap();
            index_members(node, start, ctx, class_index, TargetKind::Method);
            return;
        }
        _ => {}
    }
    let pushed = scope_of(node, start);
    if let Some(scope) = pushed {
        ctx.scope_stack.push(scope);
    }
    if matches!(node.kind(), SyntaxKind::LocalVar | SyntaxKind::Resource) {
        index_local(
            node,
            start,
            ctx,
            if node.kind() == SyntaxKind::Resource { TargetKind::Resource } else { TargetKind::Local },
        );
    }
    if node.kind() == SyntaxKind::Parameter {
        index_parameter(node, start, ctx);
    }
    if node.kind() == SyntaxKind::CatchClause {
        index_catch(node, start, ctx);
    }
    let mut offset = start;
    for child in node.children() {
        if let Some(child) = child.as_node() {
            index_node(child, offset, ctx);
        }
        offset += child.utf8_len();
    }
    if pushed.is_some() {
        ctx.scope_stack.pop();
    }
}

fn index_import(node: &GreenNode, start: u32, ctx: &mut IndexCtx<'_>) {
    let is_static = tokens(node, start).iter().any(|token| token.0 == SyntaxKind::Static);
    let Some((name_node, name_at)) = child_kind(node, start, SyntaxKind::Name) else { return };
    let (parts, star) = name_pieces(name_node, name_at);
    if parts.is_empty() {
        return;
    }
    if is_static {
        if let Some((simple, _, _)) = parts.last() {
            ctx.file.static_names.push(simple.clone());
        }
        return;
    }
    if star {
        let package = parts.iter().map(|(name, _, _)| name.clone()).collect::<Vec<_>>().join(".");
        ctx.file.stars.push(package);
    } else if let Some((simple, _, _)) = parts.last() {
        let full = parts.iter().map(|(name, _, _)| name.clone()).collect::<Vec<_>>().join(".");
        ctx.file.singles.push((simple.clone(), full));
    }
}

fn index_class(node: &GreenNode, start: u32, ctx: &mut IndexCtx<'_>) {
    let Some((name, name_start, name_end)) = direct_ident(node, start) else {
        return;
    };
    let mut nesting =
        ctx.class_stack.iter().map(|index| ctx.file.classes[*index].simple().to_string()).collect::<Vec<_>>();
    nesting.push(name);
    let mut class = ClassRec {
        path: ctx.file.path.clone(),
        package: ctx.file.package.clone(),
        nesting,
        name_start,
        name_end,
        range_start: start,
        range_end: start + node.utf8_len(),
        super_parts: None,
        interface_parts: Vec::new(),
        binary: false,
        fields: Vec::new(),
        methods: Vec::new(),
    };
    let (super_parts, interface_parts) = declared_supers(node, start);
    class.super_parts = super_parts;
    class.interface_parts = interface_parts;
    let index = ctx.file.classes.len();
    ctx.file.classes.push(class);
    ctx.class_stack.push(index);
    if node.kind() == SyntaxKind::Record
        && let Some((params, params_at)) = child_kind(node, start, SyntaxKind::ParamList)
    {
        index_components(params, params_at, ctx, index);
    }
    let mut offset = start;
    for child in node.children() {
        if let Some(child) = child.as_node() {
            index_node(child, offset, ctx);
        }
        offset += child.utf8_len();
    }
    ctx.class_stack.pop();
}

fn index_components(params: &GreenNode, start: u32, ctx: &mut IndexCtx<'_>, class_index: usize) {
    let mut offset = start;
    for child in params.children() {
        if let Some(child) = child.as_node()
            && child.kind() == SyntaxKind::Parameter
            && let Some((name, name_start, name_end)) = direct_ident(child, offset)
        {
            ctx.file.classes[class_index].fields.push(Member {
                name,
                kind: TargetKind::Field,
                start: name_start,
                end: name_end,
                declared: read_type(child, offset),
                access: read_access(child, offset),
                shape: source_type_shape(child, offset),
            });
        }
        offset += child.utf8_len();
    }
}

fn index_members(node: &GreenNode, start: u32, ctx: &mut IndexCtx<'_>, class_index: usize, kind: TargetKind) {
    if let Some((name, name_start, name_end)) = direct_ident(node, start) {
        let member = Member {
            name,
            kind,
            start: name_start,
            end: name_end,
            declared: read_type(node, start),
            access: read_access(node, start),
            shape: if kind == TargetKind::Method {
                source_method_shape(node, start)
            } else {
                source_type_shape(node, start)
            },
        };
        if kind == TargetKind::Field {
            ctx.file.classes[class_index].fields.push(member);
        } else {
            ctx.file.classes[class_index].methods.push(member);
        }
    }
    let scope = (start, start + node.utf8_len(), ScopeKind::Method);
    ctx.scope_stack.push(scope);
    let mut offset = start;
    for child in node.children() {
        if let Some(child) = child.as_node()
            && !matches!(child.kind(), SyntaxKind::Field | SyntaxKind::Method | SyntaxKind::Constructor)
        {
            index_node(child, offset, ctx);
        }
        offset += child.utf8_len();
    }
    ctx.scope_stack.pop();
}

fn index_local(node: &GreenNode, start: u32, ctx: &mut IndexCtx<'_>, kind: TargetKind) {
    let Some((scope_start, scope_end, _)) = ctx.scope_stack.last().copied() else { return };
    let declared = read_type(node, start);
    let mut offset = start;
    for child in node.children() {
        if let Some(token) = child.as_token()
            && token.kind() == SyntaxKind::Ident
        {
            ctx.file.locals.push(LocalRec {
                name: token.text().to_string(),
                kind,
                start: offset,
                end: offset + token.utf8_len(),
                scope_start,
                scope_end,
                visible_from: offset + token.utf8_len(),
                declared: declared.clone(),
                path: ctx.file.path.clone(),
            });
        }
        offset += child.utf8_len();
    }
}

fn index_catch(node: &GreenNode, start: u32, ctx: &mut IndexCtx<'_>) {
    let Some((scope_start, scope_end, _)) = ctx.scope_stack.last().copied() else { return };
    let Some((name, name_start, name_end)) = direct_ident(node, start) else { return };
    ctx.file.locals.push(LocalRec {
        name,
        kind: TargetKind::Parameter,
        start: name_start,
        end: name_end,
        scope_start,
        scope_end,
        visible_from: name_end,
        declared: read_type(node, start),
        path: ctx.file.path.clone(),
    });
}

fn index_parameter(node: &GreenNode, start: u32, ctx: &mut IndexCtx<'_>) {
    let Some((scope_start, scope_end, kind)) = ctx.scope_stack.last().copied() else { return };
    if !matches!(kind, ScopeKind::Method | ScopeKind::Lambda | ScopeKind::Catch) {
        return;
    }
    if let Some((name, name_start, name_end)) = direct_ident(node, start) {
        ctx.file.locals.push(LocalRec {
            name,
            kind: TargetKind::Parameter,
            start: name_start,
            end: name_end,
            scope_start,
            scope_end,
            visible_from: name_end,
            declared: read_type(node, start),
            path: ctx.file.path.clone(),
        });
    }
}

fn scope_of(node: &GreenNode, start: u32) -> Option<(u32, u32, ScopeKind)> {
    if node.kind() == SyntaxKind::TryStmt
        && let Some(body_end) = try_body_end(node, start)
    {
        // The header through the try block, and not the catch or finally.
        // That span is wider than the block, so a local declared in the block
        // shadows a resource of the same name.
        return Some((start, body_end, ScopeKind::Try));
    }
    let kind = match node.kind() {
        SyntaxKind::Block | SyntaxKind::ForStmt => ScopeKind::Other,
        SyntaxKind::LambdaExpr => ScopeKind::Lambda,
        SyntaxKind::CatchClause => ScopeKind::Catch,
        SyntaxKind::TryStmt => ScopeKind::Try,
        _ => return None,
    };
    Some((start, start + node.utf8_len(), kind))
}

fn try_body_end(node: &GreenNode, start: u32) -> Option<u32> {
    let mut offset = start;
    for child in node.children() {
        if let Some(token) = child.as_token()
            && token.kind() == SyntaxKind::Finally
        {
            return None;
        }
        if let Some(child) = child.as_node() {
            if child.kind() == SyntaxKind::CatchClause {
                return None;
            }
            if child.kind() == SyntaxKind::Block {
                return Some(offset + child.utf8_len());
            }
        }
        offset += child.utf8_len();
    }
    None
}

fn declared_supers(node: &GreenNode, start: u32) -> (Option<Vec<String>>, Vec<Vec<String>>) {
    let mut phase = 0u8;
    let mut extends = Vec::new();
    let mut implements = Vec::new();
    let mut offset = start;
    for child in node.children() {
        if let Some(token) = child.as_token() {
            if token.kind() == SyntaxKind::Extends {
                phase = 1;
            } else if token.kind() == SyntaxKind::Implements {
                phase = 2;
            }
        }
        if let Some(child) = child.as_node()
            && child.kind() == SyntaxKind::Type
            && let Some(name) = type_parts(child, offset)
        {
            if phase == 1 {
                extends.push(name);
            } else if phase == 2 {
                implements.push(name);
            }
        }
        offset += child.utf8_len();
    }
    let object = vec!["java".to_string(), "lang".to_string(), "Object".to_string()];
    match node.kind() {
        SyntaxKind::Interface => (Some(object), extends),
        SyntaxKind::Enum => (Some(vec!["java".to_string(), "lang".to_string(), "Enum".to_string()]), implements),
        SyntaxKind::Record => (Some(vec!["java".to_string(), "lang".to_string(), "Record".to_string()]), implements),
        _ => (Some(extends.into_iter().next().unwrap_or(object)), implements),
    }
}

fn type_parts(node: &GreenNode, start: u32) -> Option<Vec<String>> {
    let (ty, at) =
        if node.kind() == SyntaxKind::Type { (node, start) } else { child_kind(node, start, SyntaxKind::Type)? };
    let (name, name_at) = child_kind(ty, at, SyntaxKind::Name)?;
    let parts = name_strings(name, name_at);
    if parts.is_empty() { None } else { Some(parts) }
}

fn type_dotted(node: &GreenNode, start: u32) -> Option<String> {
    type_parts(node, start).map(|parts| parts.join("."))
}

fn read_access(node: &GreenNode, start: u32) -> Access {
    let Some((mods, _)) = child_kind(node, start, SyntaxKind::ModifierList) else {
        return Access::Package;
    };
    let mut seen = Vec::new();
    for (kind, _, _) in tokens(mods, 0) {
        match kind {
            SyntaxKind::Public => seen.push(Access::Public),
            SyntaxKind::Protected => seen.push(Access::Protected),
            SyntaxKind::Private => seen.push(Access::Private),
            _ => {}
        }
    }
    match seen.as_slice() {
        [] => Access::Package,
        [one] => *one,
        _ => Access::Unspecified,
    }
}

fn source_type_shape(node: &GreenNode, start: u32) -> Shape {
    match read_type(node, start) {
        Some(name) => Shape::Source(name.parts.join(".")),
        None => Shape::Unknown,
    }
}

fn source_method_shape(node: &GreenNode, start: u32) -> Shape {
    let Some((params, params_at)) = child_kind(node, start, SyntaxKind::ParamList) else {
        return Shape::Unknown;
    };
    let mut parts = Vec::new();
    let mut offset = params_at;
    for child in params.children() {
        if let Some(child) = child.as_node()
            && child.kind() == SyntaxKind::Parameter
        {
            let Some(ty) = type_dotted(child, offset).or_else(|| primitive_type(child, offset)) else {
                return Shape::Unknown;
            };
            parts.push(ty);
        }
        offset += child.utf8_len();
    }
    Shape::Source(format!("({})", parts.join(",")))
}

fn primitive_type(node: &GreenNode, start: u32) -> Option<String> {
    let (ty, at) = child_kind(node, start, SyntaxKind::Type)?;
    tokens(ty, at).into_iter().find_map(|(kind, _, _)| match kind {
        SyntaxKind::IntKw => Some("int".to_string()),
        SyntaxKind::LongKw => Some("long".to_string()),
        SyntaxKind::Short => Some("short".to_string()),
        SyntaxKind::Byte => Some("byte".to_string()),
        SyntaxKind::CharKw => Some("char".to_string()),
        SyntaxKind::BooleanKw => Some("boolean".to_string()),
        SyntaxKind::FloatKw => Some("float".to_string()),
        SyntaxKind::DoubleKw => Some("double".to_string()),
        SyntaxKind::Void => Some("void".to_string()),
        _ => None,
    })
}

fn class_from_signature(sig: &ClassSignature) -> ClassRec {
    ClassRec {
        path: sig.origin.clone(),
        package: sig.package.clone(),
        nesting: sig.nesting.clone(),
        name_start: 0,
        name_end: 0,
        range_start: 0,
        range_end: 0,
        super_parts: sig.super_class.as_ref().map(|name| name.split('.').map(str::to_string).collect()),
        interface_parts: sig.interfaces.iter().map(|name| name.split('.').map(str::to_string).collect()).collect(),
        binary: true,
        fields: sig.fields.iter().map(|member| member_from_signature(member, TargetKind::Field)).collect(),
        methods: sig
            .methods
            .iter()
            .filter(|member| member.name != "<init>" && member.kind == MemberKind::Method)
            .map(|member| member_from_signature(member, TargetKind::Method))
            .collect(),
    }
}

fn member_from_signature(member: &MemberSignature, kind: TargetKind) -> Member {
    Member {
        name: member.name.clone(),
        kind,
        start: 0,
        end: 0,
        declared: None,
        access: member.access,
        shape: Shape::Binary(member.descriptor.clone()),
    }
}

fn read_type(node: &GreenNode, start: u32) -> Option<QualName> {
    let (type_node, type_at) = child_kind(node, start, SyntaxKind::Type)?;
    let parts = if let Some((name, name_at)) = child_kind(type_node, type_at, SyntaxKind::Name) {
        name_pieces(name, name_at).0.into_iter().map(|(part, _, _)| part).collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    if parts.is_empty() { None } else { Some(QualName { parts }) }
}

fn child_kind(node: &GreenNode, start: u32, kind: SyntaxKind) -> Option<(&GreenNode, u32)> {
    let mut offset = start;
    for child in node.children() {
        if let Some(child) = child.as_node()
            && child.kind() == kind
        {
            return Some((child, offset));
        }
        offset += child.utf8_len();
    }
    None
}

fn direct_ident(node: &GreenNode, start: u32) -> Option<(String, u32, u32)> {
    let mut offset = start;
    for child in node.children() {
        if let Some(token) = child.as_token()
            && token.kind() == SyntaxKind::Ident
        {
            return Some((token.text().to_string(), offset, offset + token.utf8_len()));
        }
        offset += child.utf8_len();
    }
    None
}

fn name_strings(node: &GreenNode, start: u32) -> Vec<String> {
    name_pieces(node, start).0.into_iter().map(|(name, _, _)| name).collect()
}

fn name_pieces(node: &GreenNode, start: u32) -> (Vec<(String, u32, u32)>, bool) {
    let mut parts = Vec::new();
    let mut star = false;
    let mut offset = start;
    for child in node.children() {
        if let Some(token) = child.as_token() {
            if token.kind() == SyntaxKind::Ident {
                parts.push((token.text().to_string(), offset, offset + token.utf8_len()));
            } else if token.kind() == SyntaxKind::Star {
                star = true;
            }
        }
        offset += child.utf8_len();
    }
    (parts, star)
}

fn tokens(node: &GreenNode, start: u32) -> Vec<(SyntaxKind, u32, u32)> {
    let mut out = Vec::new();
    let mut offset = start;
    for child in node.children() {
        if let Some(token) = child.as_token() {
            out.push((token.kind(), offset, offset + token.utf8_len()));
        }
        offset += child.utf8_len();
    }
    out
}
