//! Owned view of a Java class file.
//!
//! Parsing is `cafebabe` 0.9.0 (0BSD, `forbid(unsafe_code)`). This module does
//! not call a JVM. IntelliJ's compiled PSI is not reused: at commit
//! `840d3be11dfdeada533d05052008b48816b6cc4e`, `StubBuildingVisitor` extends
//! ASM `ClassVisitor` and builds stubs that `ClsClassImpl` then exposes as
//! `PsiClass`. The binder in this crate consumes [`ClassSignature`] the same
//! way it consumes a parsed source type. That is a shared index, not a port
//! of ASM or of the stub builder.

use std::fmt;
use std::fs;
use std::path::Path;

use cafebabe::attributes::AttributeData;

/// Major versions this reader accepts: 45 (Java 1.1) through 71 (Java 27).
const MIN_MAJOR: u16 = 45;
const MAX_MAJOR: u16 = 71;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    Public,
    Protected,
    Private,
    /// No access flag. Same package only.
    Package,
    /// More than one of public, protected, and private.
    Unspecified,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemberKind {
    Field,
    Method,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemberSignature {
    pub name: String,
    pub kind: MemberKind,
    /// JVM field or method descriptor, such as `I` or `()V`.
    pub descriptor: String,
    /// Raw `Signature` attribute. Generic types are not instantiated.
    pub signature: Option<String>,
    pub access: Access,
}

/// One successfully parsed class file and the path it was registered under.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClassSignature {
    pub origin: String,
    /// Internal name with `/`, such as `pkg/Outer$Inner`.
    pub binary_name: String,
    /// `binary_name` with `/` and `$` turned into `.`.
    ///
    /// `$` is treated as javac's nesting separator. A binary name that uses
    /// `$` for another reason is misread.
    pub dotted_name: String,
    pub package: String,
    pub nesting: Vec<String>,
    pub super_class: Option<String>,
    pub interfaces: Vec<String>,
    pub signature: Option<String>,
    pub major: u16,
    pub minor: u16,
    pub access: Access,
    pub fields: Vec<MemberSignature>,
    pub methods: Vec<MemberSignature>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClassErrorKind {
    Truncated,
    BadMagic,
    BadConstantPool,
    UnsupportedVersion,
    Io,
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClassError {
    pub origin: String,
    pub kind: ClassErrorKind,
    pub detail: String,
}

impl fmt::Display for ClassError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {:?}: {}", self.origin, self.kind, self.detail)
    }
}

impl std::error::Error for ClassError {}

/// Parse `bytes` as a class file. Truncation, a bad magic, a bad constant
/// pool, and a version outside 45..=71 are errors. This does not panic on
/// those inputs.
pub fn parse_class_bytes(origin: &str, bytes: &[u8]) -> Result<ClassSignature, ClassError> {
    let mut options = cafebabe::ParseOptions::default();
    options.parse_bytecode(false);
    let class =
        cafebabe::parse_class_with_options(bytes, &options).map_err(|err| classify(origin, &err.to_string()))?;
    if class.major_version < MIN_MAJOR || class.major_version > MAX_MAJOR {
        return Err(ClassError {
            origin: origin.to_string(),
            kind: ClassErrorKind::UnsupportedVersion,
            detail: format!("major {} minor {}", class.major_version, class.minor_version),
        });
    }
    if class.major_version >= 56 && class.minor_version != 0 && class.minor_version != 65535 {
        return Err(ClassError {
            origin: origin.to_string(),
            kind: ClassErrorKind::UnsupportedVersion,
            detail: format!("major {} minor {}", class.major_version, class.minor_version),
        });
    }
    Ok(signature_from(origin, &class))
}

/// Read every `*.class` file under `root`. A parse error or an I/O error
/// fails the whole call; nothing from this call is returned. Symlinks are
/// not followed. This does not open JARs.
pub(crate) fn read_class_directory(root: &Path) -> Result<Vec<ClassSignature>, ClassError> {
    let mut out = Vec::new();
    walk_class_dir(root, &mut out)?;
    Ok(out)
}

fn walk_class_dir(dir: &Path, out: &mut Vec<ClassSignature>) -> Result<(), ClassError> {
    let entries = fs::read_dir(dir).map_err(|err| ClassError {
        origin: dir.display().to_string(),
        kind: ClassErrorKind::Io,
        detail: err.to_string(),
    })?;
    for entry in entries {
        let entry = entry.map_err(|err| ClassError {
            origin: dir.display().to_string(),
            kind: ClassErrorKind::Io,
            detail: err.to_string(),
        })?;
        let path = entry.path();
        let kind = entry.file_type().map_err(|err| ClassError {
            origin: path.display().to_string(),
            kind: ClassErrorKind::Io,
            detail: err.to_string(),
        })?;
        if kind.is_dir() {
            walk_class_dir(&path, out)?;
            continue;
        }
        if !kind.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !name.ends_with(".class") {
            continue;
        }
        let bytes = fs::read(&path).map_err(|err| ClassError {
            origin: path.display().to_string(),
            kind: ClassErrorKind::Io,
            detail: err.to_string(),
        })?;
        out.push(parse_class_bytes(&path.display().to_string(), &bytes)?);
    }
    Ok(())
}

fn signature_from(origin: &str, class: &cafebabe::ClassFile<'_>) -> ClassSignature {
    let binary_name = class.this_class.to_string();
    let (package, nesting) = split_binary(&binary_name);
    let dotted_name = dotted_binary(&binary_name);
    ClassSignature {
        origin: origin.to_string(),
        binary_name,
        dotted_name,
        package,
        nesting,
        super_class: class.super_class.as_ref().map(|name| dotted_binary(name)),
        interfaces: class.interfaces.iter().map(|name| dotted_binary(name)).collect(),
        signature: signature_attr(&class.attributes),
        major: class.major_version,
        minor: class.minor_version,
        access: class_access(class.access_flags),
        fields: class
            .fields
            .iter()
            .map(|field| MemberSignature {
                name: field.name.to_string(),
                kind: MemberKind::Field,
                descriptor: field.descriptor.to_string(),
                signature: signature_attr(&field.attributes),
                access: field_access(field.access_flags),
            })
            .collect(),
        methods: class
            .methods
            .iter()
            .filter(|method| method.name != "<clinit>")
            .map(|method| MemberSignature {
                name: method.name.to_string(),
                kind: MemberKind::Method,
                descriptor: method.descriptor.to_string(),
                signature: signature_attr(&method.attributes),
                access: method_access(method.access_flags),
            })
            .collect(),
    }
}

fn signature_attr(attributes: &[cafebabe::attributes::AttributeInfo<'_>]) -> Option<String> {
    attributes.iter().find_map(|attr| match &attr.data {
        AttributeData::Signature(text) => Some(text.to_string()),
        _ => None,
    })
}

fn split_binary(binary: &str) -> (String, Vec<String>) {
    let (package, simple) = match binary.rfind('/') {
        Some(index) => (binary[..index].replace('/', "."), &binary[index + 1..]),
        None => (String::new(), binary),
    };
    let nesting = simple.split('$').filter(|part| !part.is_empty()).map(str::to_string).collect();
    (package, nesting)
}

pub(crate) fn dotted_binary(binary: &str) -> String {
    binary.replace(['/', '$'], ".")
}

fn class_access(flags: cafebabe::ClassAccessFlags) -> Access {
    // `ClassAccessFlags` does not name PRIVATE or PROTECTED. Nested classes
    // still store those bits in the same u16.
    let bits = flags.bits();
    one_access(bits & 0x0001 != 0, bits & 0x0004 != 0, bits & 0x0002 != 0)
}

fn field_access(flags: cafebabe::FieldAccessFlags) -> Access {
    one_access(
        flags.contains(cafebabe::FieldAccessFlags::PUBLIC),
        flags.contains(cafebabe::FieldAccessFlags::PROTECTED),
        flags.contains(cafebabe::FieldAccessFlags::PRIVATE),
    )
}

fn method_access(flags: cafebabe::MethodAccessFlags) -> Access {
    one_access(
        flags.contains(cafebabe::MethodAccessFlags::PUBLIC),
        flags.contains(cafebabe::MethodAccessFlags::PROTECTED),
        flags.contains(cafebabe::MethodAccessFlags::PRIVATE),
    )
}

fn one_access(public: bool, protected: bool, private: bool) -> Access {
    match (public, protected, private) {
        (true, false, false) => Access::Public,
        (false, true, false) => Access::Protected,
        (false, false, true) => Access::Private,
        (false, false, false) => Access::Package,
        _ => Access::Unspecified,
    }
}

fn classify(origin: &str, detail: &str) -> ClassError {
    let kind = if detail.contains("end of stream") {
        ClassErrorKind::Truncated
    } else if detail.contains("magic") {
        ClassErrorKind::BadMagic
    } else if detail.contains("constant pool")
        || detail.contains("Constant pool")
        || detail.contains("cp ")
        || detail.contains("tag")
    {
        ClassErrorKind::BadConstantPool
    } else if detail.contains("version") {
        ClassErrorKind::UnsupportedVersion
    } else {
        ClassErrorKind::Other
    };
    ClassError { origin: origin.to_string(), kind, detail: detail.to_string() }
}
