use std::collections::{HashMap, HashSet};

use crate::compiler::ast::{self as AS, Block, Decl, Expr, Stmt};
use crate::compiler::attrs::{parse_attr_args, parse_default_expr};
use crate::compiler::diagnostics::{Diagnostic, Reporter};
use crate::compiler::resolve::{ResolvedKind, ResolvedProgram, lookup_symbol_kind};
use crate::compiler::source::SourceFile;
use crate::compiler::stdlib::StdlibIndex;

pub mod attrs;
pub mod capability;
pub mod const_eval;
mod constraint;
mod defassign;
pub mod effects;
pub mod escape_results;
mod exhaustiveness;
pub mod lifetime;
pub mod nll;

use defassign::InitEnv;
use effects::EffectEnv;
use lifetime::LifetimeEnv;

pub use escape_results::{
    AllocStrategy, EscapeAnalysisResults, FunctionEscapeInfo, LocalEscapeInfo,
};

use lifetime::{BorrowMode, RegionId};

/// Region representation for lifetime inference.
/// Used internally by the borrow checker - never exposed in user-facing types.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Region {
    /// A concrete region with a known ID
    Concrete(RegionId),
    /// An inference variable for an unknown region (solved during constraint solving)
    Var(RegionVar),
    /// Static region - lives for entire program (for constants)
    Static,
}

/// Inference variable for an unknown region.
/// Created during type checking and unified during constraint solving.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RegionVar(pub u32);

impl RegionVar {
    pub fn new(id: u32) -> Self {
        RegionVar(id)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Ty {
    Int,
    Float,
    Bool,
    String,
    Char,
    Bytes,
    Void,
    Function(Vec<Ty>, Box<Ty>), // params, ret
    Named(Vec<String>),
    /// Generic type with type arguments, e.g., Task<String>, Receiver<Message>
    /// The path is the base type name, args are the type arguments
    Generic {
        path: Vec<String>,
        args: Vec<Ty>,
    },
    /// Tuple type for multiple values, e.g., (Int, String)
    Tuple(Vec<Ty>),
    /// Never type - for expressions that never return (e.g., throw, panic, infinite loop)
    Never,
    /// Internal reference type for borrow checking.
    /// Never appears in user code (no `&` syntax) - only used internally by the compiler.
    /// Represents a borrowed reference to another value.
    Ref {
        /// The type being borrowed
        inner: Box<Ty>,
        /// Whether this is a shared or exclusive borrow
        mode: BorrowMode,
        /// The lifetime region of this borrow
        region: Region,
    },
    Unknown,
}

impl std::fmt::Display for Ty {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Ty::Int => write!(f, "Int"),
            Ty::Float => write!(f, "Float"),
            Ty::Bool => write!(f, "Bool"),
            Ty::String => write!(f, "String"),
            Ty::Char => write!(f, "Char"),
            Ty::Bytes => write!(f, "Bytes"),
            Ty::Void => write!(f, "Void"),
            Ty::Function(params, ret) => {
                write!(f, "(")?;
                for (i, p) in params.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", p)?;
                }
                write!(f, ") -> {}", ret)
            }
            Ty::Named(path) => write!(f, "{}", path.join(".")),
            Ty::Generic { path, args } => {
                write!(f, "{}", path.join("."))?;
                if !args.is_empty() {
                    write!(f, "<")?;
                    for (i, a) in args.iter().enumerate() {
                        if i > 0 {
                            write!(f, ", ")?;
                        }
                        write!(f, "{}", a)?;
                    }
                    write!(f, ">")?;
                }
                Ok(())
            }
            Ty::Tuple(elems) => {
                write!(f, "(")?;
                for (i, e) in elems.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", e)?;
                }
                write!(f, ")")
            }
            Ty::Never => write!(f, "Never"),
            Ty::Ref {
                inner,
                mode,
                region,
            } => {
                // Internal representation for diagnostics - users never see this syntax
                let mode_str = match mode {
                    BorrowMode::Shared => "&",
                    BorrowMode::Exclusive => "&mut ",
                };
                let region_str = match region {
                    Region::Concrete(id) => format!("'r{}", id.0),
                    Region::Var(v) => format!("'?{}", v.0),
                    Region::Static => "'static".to_string(),
                };
                write!(f, "{}{} {}", mode_str, region_str, inner)
            }
            Ty::Unknown => write!(f, "Unknown"),
        }
    }
}

/// Check if a type name (case-insensitive) is a primitive type.
/// Returns Some(Ty) if primitive, None otherwise.
fn try_primitive_ty(name: &str) -> Option<Ty> {
    let base = name.to_ascii_lowercase();

    // Helper: matches suffix that is one of 8,16,32,64,128
    let is_width = |s: &str| matches!(s, "8" | "16" | "32" | "64" | "128");

    // int-family: signed/unsigned and aliases
    if base == "int" || base == "short" || base == "long" {
        return Some(Ty::Int);
    }
    if base == "i32" || base == "i64" || base == "i16" || base == "i8" || base == "i128" {
        return Some(Ty::Int);
    }
    if base == "u8" || base == "u16" || base == "u32" || base == "u64" || base == "u128" {
        return Some(Ty::Int);
    }
    if base.starts_with("int") {
        let w = &base[3..];
        if !w.is_empty() && is_width(w) {
            return Some(Ty::Int);
        }
    }
    if base.starts_with("uint") {
        let w = &base[4..];
        if !w.is_empty() && is_width(w) {
            return Some(Ty::Int);
        }
    }

    // float-family
    if base == "float"
        || base == "double"
        || base == "f32"
        || base == "f64"
        || base == "float32"
        || base == "float64"
    {
        return Some(Ty::Float);
    }

    // Other primitives
    if base == "bool" || base == "boolean" {
        return Some(Ty::Bool);
    }
    if base == "string" {
        return Some(Ty::String);
    }
    if base == "char" {
        return Some(Ty::Char);
    }
    if base == "bytes" {
        return Some(Ty::Bytes);
    }
    if base == "void" {
        return Some(Ty::Void);
    }
    if base == "never" {
        return Some(Ty::Never);
    }

    None
}

/// Convert a NamePath (with potential type arguments) to a Ty.
/// This properly handles generic types like List<Int>, Map<String, Int>, Optional<T>, etc.
fn map_name_to_ty(np: &AS::NamePath) -> Ty {
    let parts: Vec<String> = np.path.iter().map(|i| i.0.clone()).collect();
    if parts.is_empty() {
        return Ty::Unknown;
    }

    // Check if this is a primitive type (primitives cannot have type arguments)
    if np.type_args.is_empty() {
        if let Some(prim) = try_primitive_ty(parts.last().unwrap()) {
            return prim;
        }
    }

    // If there are type arguments, create a Generic type
    if !np.type_args.is_empty() {
        let args: Vec<Ty> = np.type_args.iter().map(map_name_to_ty).collect();
        return Ty::Generic { path: parts, args };
    }

    // Otherwise a named (user) type without type arguments
    Ty::Named(parts)
}

// --- Optional<T> Type Utilities ---

/// Check if a type is Optional<T>
fn is_optional_ty(ty: &Ty) -> bool {
    matches!(ty, Ty::Generic { path, args } if path.last().map(|s| s.as_str()) == Some("Optional") && args.len() == 1)
}

/// Extract the inner type T from Optional<T>, or None if not an Optional
fn unwrap_optional_ty(ty: &Ty) -> Option<&Ty> {
    match ty {
        Ty::Generic { path, args }
            if path.last().map(|s| s.as_str()) == Some("Optional") && args.len() == 1 =>
        {
            Some(&args[0])
        }
        _ => None,
    }
}

/// Wrap a type in Optional<T>
fn wrap_in_optional(ty: Ty) -> Ty {
    Ty::Generic {
        path: vec!["Optional".to_string()],
        args: vec![ty],
    }
}

/// Check if a type is Result<T, E>
fn is_result_ty(ty: &Ty) -> bool {
    matches!(ty, Ty::Generic { path, args } if path.last().map(|s| s.as_str()) == Some("Result") && args.len() == 2)
}

/// Extract (Ok type, Err type) from Result<T, E>, or None if not a Result
fn unwrap_result_ty(ty: &Ty) -> Option<(&Ty, &Ty)> {
    match ty {
        Ty::Generic { path, args }
            if path.last().map(|s| s.as_str()) == Some("Result") && args.len() == 2 =>
        {
            Some((&args[0], &args[1]))
        }
        _ => None,
    }
}

/// Check if a type is List<T>
fn is_list_ty(ty: &Ty) -> bool {
    matches!(ty, Ty::Generic { path, args } if path.last().map(|s| s.as_str()) == Some("List") && args.len() == 1)
}

/// Extract the element type from List<T>
fn unwrap_list_ty(ty: &Ty) -> Option<&Ty> {
    match ty {
        Ty::Generic { path, args }
            if path.last().map(|s| s.as_str()) == Some("List") && args.len() == 1 =>
        {
            Some(&args[0])
        }
        _ => None,
    }
}

/// Check if a type is Map<K, V>
fn is_map_ty(ty: &Ty) -> bool {
    matches!(ty, Ty::Generic { path, args } if path.last().map(|s| s.as_str()) == Some("Map") && args.len() == 2)
}

/// Extract (key type, value type) from Map<K, V>
fn unwrap_map_ty(ty: &Ty) -> Option<(&Ty, &Ty)> {
    match ty {
        Ty::Generic { path, args }
            if path.last().map(|s| s.as_str()) == Some("Map") && args.len() == 2 =>
        {
            Some((&args[0], &args[1]))
        }
        _ => None,
    }
}

/// Check if a type is Task<T> (async task)
fn is_task_ty(ty: &Ty) -> bool {
    matches!(ty, Ty::Generic { path, args } if path.last().map(|s| s.as_str()) == Some("Task") && args.len() == 1)
}

/// Extract the result type from Task<T>
fn unwrap_task_ty(ty: &Ty) -> Option<&Ty> {
    match ty {
        Ty::Generic { path, args }
            if path.last().map(|s| s.as_str()) == Some("Task") && args.len() == 1 =>
        {
            Some(&args[0])
        }
        _ => None,
    }
}

// Field metadata: (type, visibility, is_final, default_expr)
// Defined early as it's used by lookup_struct_field
type FieldMetadataRef<'a> = &'a (AS::NamePath, AS::Visibility, bool, Option<String>);

/// Look up a field in a struct by type path and field name.
/// Returns the field metadata if found.
fn lookup_struct_field<'a>(
    env: &'a Env,
    type_path: &[String],
    field_name: &str,
) -> Option<FieldMetadataRef<'a>> {
    if type_path.is_empty() {
        return None;
    }

    let type_name = type_path.last().unwrap().clone();
    let pkg = if type_path.len() > 1 {
        type_path[..type_path.len() - 1].join(".")
    } else {
        env.current_pkg.clone().unwrap_or_default()
    };

    // Look up in struct_fields index
    if let Some(fields) = env.struct_fields.get(&(pkg.clone(), type_name.clone())) {
        return fields.get(field_name);
    }

    // Try with star-imported packages
    for star_pkg in env.star_import_pkgs.iter() {
        if let Some(fields) = env
            .struct_fields
            .get(&(star_pkg.clone(), type_name.clone()))
        {
            return fields.get(field_name);
        }
    }

    None
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NumKind {
    IntSigned,
    IntUnsigned,
    Float,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct NumTy {
    kind: NumKind,
    bits: u16,
}

fn numty_from_namepath(np: &AS::NamePath) -> Option<NumTy> {
    let parts: Vec<String> = np.path.iter().map(|i| i.0.clone()).collect();
    if parts.is_empty() {
        return None;
    }
    let base = parts.last().unwrap().to_ascii_lowercase();

    // Signed ints
    if base == "int" || base == "i32" {
        return Some(NumTy {
            kind: NumKind::IntSigned,
            bits: 32,
        });
    }
    if base == "short" || base == "i16" {
        return Some(NumTy {
            kind: NumKind::IntSigned,
            bits: 16,
        });
    }
    if base == "long" || base == "i64" {
        return Some(NumTy {
            kind: NumKind::IntSigned,
            bits: 64,
        });
    }
    if base == "i8" {
        return Some(NumTy {
            kind: NumKind::IntSigned,
            bits: 8,
        });
    }
    if base == "i128" {
        return Some(NumTy {
            kind: NumKind::IntSigned,
            bits: 128,
        });
    }
    if base.starts_with("int") {
        let w = &base[3..];
        if let Ok(n) = w.parse::<u16>() {
            return Some(NumTy {
                kind: NumKind::IntSigned,
                bits: n,
            });
        }
    }

    // Unsigned ints
    if base == "u8" {
        return Some(NumTy {
            kind: NumKind::IntUnsigned,
            bits: 8,
        });
    }
    if base == "u16" {
        return Some(NumTy {
            kind: NumKind::IntUnsigned,
            bits: 16,
        });
    }
    if base == "u32" {
        return Some(NumTy {
            kind: NumKind::IntUnsigned,
            bits: 32,
        });
    }
    if base == "u64" {
        return Some(NumTy {
            kind: NumKind::IntUnsigned,
            bits: 64,
        });
    }
    if base == "u128" {
        return Some(NumTy {
            kind: NumKind::IntUnsigned,
            bits: 128,
        });
    }
    if base.starts_with("uint") {
        let w = &base[4..];
        if let Ok(n) = w.parse::<u16>() {
            return Some(NumTy {
                kind: NumKind::IntUnsigned,
                bits: n,
            });
        }
    }

    // Floats: map 'float' to 64-bit (VM uses f64 for Float); f32 explicitly via f32/float32
    if base == "float" || base == "double" || base == "f64" || base == "float64" {
        return Some(NumTy {
            kind: NumKind::Float,
            bits: 64,
        });
    }
    if base == "f32" || base == "float32" {
        return Some(NumTy {
            kind: NumKind::Float,
            bits: 32,
        });
    }

    None
}

#[derive(Clone, Debug)]
enum CollectionKind {
    List(Box<Ty>),
    Map(Box<Ty>, Box<Ty>),
}

/// Move state for a local variable, tracking partial and conditional moves
#[derive(Clone, Debug, PartialEq, Eq)]
enum MoveState {
    /// Variable is fully available (not moved)
    Available,
    /// Variable is definitely fully moved
    FullyMoved,
    /// Variable may or may not be moved depending on control flow path
    /// (e.g., moved in one branch of an if/else but not the other)
    ConditionallyMoved,
    /// Some fields are moved, others are available.
    /// The set contains the names of moved fields.
    PartiallyMoved(HashSet<String>),
}

impl Default for MoveState {
    fn default() -> Self {
        MoveState::Available
    }
}

impl MoveState {
    /// Check if the variable is definitely moved (fully or any field)
    fn is_definitely_moved(&self) -> bool {
        matches!(self, MoveState::FullyMoved)
    }

    /// Check if a specific field is moved
    fn is_field_moved(&self, field: &str) -> bool {
        match self {
            MoveState::FullyMoved => true,
            MoveState::PartiallyMoved(fields) => fields.contains(field),
            MoveState::ConditionallyMoved => true, // Conservative: treat as moved
            MoveState::Available => false,
        }
    }

    /// Check if the variable might be moved (fully, partially, or conditionally)
    #[allow(dead_code)]
    fn might_be_moved(&self) -> bool {
        !matches!(self, MoveState::Available)
    }

    /// Mark a field as moved
    fn move_field(&mut self, field: &str) {
        match self {
            MoveState::Available => {
                let mut fields = HashSet::new();
                fields.insert(field.to_string());
                *self = MoveState::PartiallyMoved(fields);
            }
            MoveState::PartiallyMoved(fields) => {
                fields.insert(field.to_string());
            }
            // Already fully or conditionally moved - no change
            MoveState::FullyMoved | MoveState::ConditionallyMoved => {}
        }
    }

    /// Mark the entire variable as fully moved
    fn move_fully(&mut self) {
        *self = MoveState::FullyMoved;
    }

    /// Reset to available (e.g., after reassignment)
    fn reset(&mut self) {
        *self = MoveState::Available;
    }

    /// Join two move states (for control flow merge points)
    /// Conservative: if moved on any path, result reflects that
    fn join(&self, other: &MoveState) -> MoveState {
        match (self, other) {
            // Both available -> available
            (MoveState::Available, MoveState::Available) => MoveState::Available,

            // Both fully moved -> fully moved
            (MoveState::FullyMoved, MoveState::FullyMoved) => MoveState::FullyMoved,

            // One available, one fully moved -> conditionally moved
            (MoveState::Available, MoveState::FullyMoved)
            | (MoveState::FullyMoved, MoveState::Available) => MoveState::ConditionallyMoved,

            // Any conditional involvement -> conditional
            (MoveState::ConditionallyMoved, _) | (_, MoveState::ConditionallyMoved) => {
                MoveState::ConditionallyMoved
            }

            // Partial moves: union the moved fields
            (MoveState::PartiallyMoved(a), MoveState::PartiallyMoved(b)) => {
                let mut union = a.clone();
                union.extend(b.iter().cloned());
                MoveState::PartiallyMoved(union)
            }

            // Partial + available -> partial stays (fields moved on one path)
            (MoveState::PartiallyMoved(fields), MoveState::Available)
            | (MoveState::Available, MoveState::PartiallyMoved(fields)) => {
                // When joining with Available, the partial move becomes conditional
                // for those specific fields, but we track it as partial
                MoveState::PartiallyMoved(fields.clone())
            }

            // Partial + fully moved -> conditional (conservative)
            (MoveState::PartiallyMoved(_), MoveState::FullyMoved)
            | (MoveState::FullyMoved, MoveState::PartiallyMoved(_)) => {
                MoveState::ConditionallyMoved
            }
        }
    }

    /// Get set of moved fields (empty if not partially moved)
    #[allow(dead_code)]
    fn moved_fields(&self) -> HashSet<String> {
        match self {
            MoveState::PartiallyMoved(fields) => fields.clone(),
            _ => HashSet::new(),
        }
    }
}

/// Actions that may conflict with existing borrows.
/// Used for borrow conflict detection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BorrowAction {
    /// Creating a shared (immutable) borrow
    SharedBorrow,
    /// Creating an exclusive (mutable) borrow
    ExclusiveBorrow,
    /// Moving the value (transfers ownership)
    Move,
}

#[derive(Clone, Debug)]
struct LocalInfo {
    ty: Ty,
    is_final: bool,
    initialized: bool,
    /// Legacy field - kept for backward compatibility during transition
    /// Use move_state for new code
    moved: bool,
    /// Detailed move tracking: full moves, partial moves, conditional moves
    move_state: MoveState,
    // Optional numeric representation for width/sign-aware policies
    num: Option<NumTy>,
    col_kind: Option<CollectionKind>,
    // Drop tracking: whether this type needs cleanup via deinit
    needs_drop: bool,
    // Qualified type name for drop resolution (e.g., "pkg.StructName")
    drop_ty_name: Option<String>,
    #[allow(dead_code)]
    // Declaration order for reverse-order drop (lower = declared earlier)
    decl_order: u32,
}

type ConcTraits = std::collections::HashMap<(String, String), (bool, bool, bool)>; // (pkg, name) -> (Sendable, Shareable, UnwindSafe)

// Field metadata: (type, visibility, is_final, default_expr)
type FieldMetadata = (AS::NamePath, AS::Visibility, bool, Option<String>);

type StructFieldsIndex = std::collections::HashMap<
    (String, String),                                 // (pkg, struct)
    std::collections::HashMap<String, FieldMetadata>, // field -> (type, vis, is_final, default)
>;

fn is_excl_borrowed(env: &Env, name: &str) -> bool {
    for scope in &env.excl_borrows {
        if scope.contains(name) {
            return true;
        }
    }
    false
}

/// Kind of deinit for a type
#[derive(Debug, Clone, PartialEq)]
pub enum DeinitKind {
    /// Explicit deinit function in companion module (e.g., "FooFns")
    Explicit(String),
    /// Synthetic/auto-generated deinit - drops each field in reverse order
    /// Contains list of (field_name, field_type_key) pairs that need dropping
    Synthetic(Vec<(String, String)>),
}

// Types that need drop are named types with a deinit function (explicit or synthetic).
// Key: (pkg, type_name), Value: kind of deinit
type TypesNeedingDrop = std::collections::HashMap<(String, String), DeinitKind>;

/// FFI ownership attribute on an extern function
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FfiOwnership {
    /// No explicit ownership attribute - default behavior
    None,
    /// @ffi_owned - Arth takes ownership of returned value (must cleanup)
    Owned,
    /// @ffi_borrowed - Value is borrowed (read-only, no cleanup)
    Borrowed,
    /// @ffi_transfers - Arth transfers ownership to C (moved, no cleanup)
    Transfers,
}

/// Information about an extern function
#[derive(Debug, Clone)]
pub struct ExternFuncInfo {
    /// Parameter types for FFI safety checking
    pub param_types: Vec<AS::NamePath>,
    /// Return type (None = void)
    pub ret_type: Option<AS::NamePath>,
    /// FFI ownership attribute
    pub ownership: FfiOwnership,
}

// Extern functions index: (pkg, name) -> extern function info
// Calling extern functions requires unsafe context
type ExternFuncsIndex = std::collections::HashMap<(String, String), ExternFuncInfo>;

// Unsafe functions index: (pkg, module_opt, name) -> true
// module_opt is None for top-level functions, Some(module_name) for module functions
// Calling unsafe functions requires unsafe context
type UnsafeFuncsIndex = std::collections::HashSet<(String, Option<String>, String)>;

/// Implements index: tracks which types implement which interfaces.
/// Key: (pkg, type_name)
/// Value: Set of interface types implemented by this type.
type ImplementsIndex = std::collections::HashMap<(String, String), HashSet<Ty>>;

/// Copy types index: tracks which types implement the Copy trait.
/// Key: (pkg, type_name)
/// Value: CopyKind indicating how Copy was implemented
#[derive(Clone, Debug, PartialEq, Eq)]
enum CopyKind {
    /// Explicitly declared via `implements Copy`
    Explicit,
    /// Auto-derived because all fields are Copy types
    Derived,
}

type CopyTypesIndex = std::collections::HashMap<(String, String), CopyKind>;

#[derive(Clone)]
struct Env {
    stack: Vec<HashMap<String, LocalInfo>>, // innermost at end
    // Borrow tracking per scope:
    // - excl_borrows: names under exclusive borrow (borrowMut)
    // - shared_borrows: names with active shared borrows -> list of region IDs
    // - prov_borrows: provider names with tied borrows
    excl_borrows: Vec<std::collections::HashSet<String>>, // names under exclusive borrow
    shared_borrows: Vec<HashMap<String, Vec<RegionId>>>,  // names -> active shared borrow regions
    prov_borrows: Vec<std::collections::HashSet<String>>, // provider names with tied borrows
    current_pkg: Option<String>,
    current_module: Option<String>,
    conc: std::sync::Arc<ConcTraits>,
    in_async: bool,
    in_main: bool, // True when checking main() - allows await for implicit async entry
    // Function signature lookups
    free_fns: std::sync::Arc<std::collections::HashMap<(String, String), Vec<AS::FuncSig>>>, // (pkg, name) -> overloads
    module_fns:
        std::sync::Arc<std::collections::HashMap<(String, String, String), Vec<AS::FuncSig>>>, // (pkg, module, name) -> overloads
    // Struct field type map for member typing
    struct_fields: std::sync::Arc<StructFieldsIndex>,
    // Imported modules visible in this file: ModuleName -> package path
    imported_modules: std::sync::Arc<std::collections::HashMap<String, String>>,
    // Packages brought into scope via star import (e.g., import pkg.*;)
    star_import_pkgs: std::sync::Arc<Vec<String>>,
    // Type aliases visible to this file: (pkg, alias) -> target NamePath
    type_aliases: std::sync::Arc<std::collections::HashMap<(String, String), AS::NamePath>>,
    // Enum variants: (pkg, enum, variant) -> payload types (AST NamePaths)
    enum_variants:
        std::sync::Arc<std::collections::HashMap<(String, String, String), Vec<AS::NamePath>>>,
    // Declared return type for the current function (Void for no return)
    expected_ret: Ty,
    // Definite assignment tracking for path-sensitive initialization analysis
    init_env: InitEnv,
    // Lifetime inference: tracks active borrows and their regions
    lifetime_env: LifetimeEnv,
    // Effect system: tracks mutation effects and shared state access
    effect_env: EffectEnv,
    // Types that need drop (have deinit): (pkg, type_name) -> deinit module name
    types_needing_drop: std::sync::Arc<TypesNeedingDrop>,
    // Counter for local variable declaration order (for reverse-order drops)
    decl_order_counter: u32,
    // Depth of unsafe blocks we are currently inside (0 = safe context)
    // This is initialized to 1 if the current function is marked as unsafe
    unsafe_depth: u32,
    // Extern functions: calling these requires unsafe context
    extern_funcs: std::sync::Arc<ExternFuncsIndex>,
    // Unsafe functions: calling these requires unsafe context
    unsafe_funcs: std::sync::Arc<UnsafeFuncsIndex>,
    // Whether we are currently inside a finally block
    // Finally blocks have restrictions: cannot return with value, cannot move values out
    in_finally: bool,
    // Types that implement Copy: either explicitly via `implements Copy` or auto-derived
    copy_types: std::sync::Arc<CopyTypesIndex>,
    // Generic type parameters in scope: name -> bound (if any)
    // Used during type checking of generic function bodies
    type_params: HashMap<String, Option<AS::NamePath>>,
    // Program-wide index of interface implementations
    implements: std::sync::Arc<ImplementsIndex>,
    // Counter for generating unique region IDs for shared borrows
    next_region_id: u32,
    // Stack of loop/switch labels currently in scope for break/continue validation
    // Each entry is (label_name, is_loop) where is_loop=true for while/for, false for switch
    loop_labels: Vec<(String, bool)>,
    // Depth of finally blocks containing each label (for labeled break/continue in finally validation)
    // Maps label name to the finally depth when it was declared
    label_finally_depth: HashMap<String, u32>,
    // Current finally block nesting depth
    finally_depth: u32,
}

impl Env {
    fn new(
        current_pkg: Option<String>,
        current_module: Option<String>,
        conc: std::sync::Arc<ConcTraits>,
        in_async: bool,
        in_main: bool, // True when checking main() - allows await
        in_unsafe_fn: bool,
        free_fns: std::sync::Arc<std::collections::HashMap<(String, String), Vec<AS::FuncSig>>>,
        module_fns: std::sync::Arc<
            std::collections::HashMap<(String, String, String), Vec<AS::FuncSig>>,
        >,
        struct_fields: std::sync::Arc<StructFieldsIndex>,
        imported_modules: std::sync::Arc<std::collections::HashMap<String, String>>,
        star_import_pkgs: std::sync::Arc<Vec<String>>,
        type_aliases: std::sync::Arc<std::collections::HashMap<(String, String), AS::NamePath>>,
        enum_variants: std::sync::Arc<
            std::collections::HashMap<(String, String, String), Vec<AS::NamePath>>,
        >,
        types_needing_drop: std::sync::Arc<TypesNeedingDrop>,
        extern_funcs: std::sync::Arc<ExternFuncsIndex>,
        unsafe_funcs: std::sync::Arc<UnsafeFuncsIndex>,
        copy_types: std::sync::Arc<CopyTypesIndex>,
        implements: std::sync::Arc<ImplementsIndex>,
    ) -> Self {
        Env {
            stack: vec![HashMap::new()],
            excl_borrows: vec![Default::default()],
            shared_borrows: vec![HashMap::new()],
            prov_borrows: vec![Default::default()],
            current_pkg,
            current_module,
            conc,
            in_async,
            in_main,
            free_fns,
            module_fns,
            struct_fields,
            imported_modules,
            star_import_pkgs,
            type_aliases,
            enum_variants,
            expected_ret: Ty::Void,
            init_env: InitEnv::new(),
            // Enable NLL in the default typecheck path.
            lifetime_env: LifetimeEnv::with_nll(),
            effect_env: EffectEnv::new(),
            types_needing_drop,
            decl_order_counter: 0,
            unsafe_depth: if in_unsafe_fn { 1 } else { 0 },
            extern_funcs,
            unsafe_funcs,
            in_finally: false,
            copy_types,
            type_params: HashMap::new(),
            implements,
            next_region_id: 0,
            loop_labels: Vec::new(),
            label_finally_depth: HashMap::new(),
            finally_depth: 0,
        }
    }

    /// Set type parameters for a generic function being type-checked
    fn set_type_params(&mut self, generics: &[AS::GenericParam]) {
        self.type_params.clear();
        for gp in generics {
            self.type_params.insert(gp.name.0.clone(), gp.bound.clone());
        }
    }

    /// Check if a name refers to a type parameter in scope
    fn is_type_param(&self, name: &str) -> bool {
        self.type_params.contains_key(name)
    }

    /// Map a NamePath to Ty, respecting type parameters in scope
    fn map_name_to_ty_with_params(&self, np: &AS::NamePath) -> Ty {
        let parts: Vec<String> = np.path.iter().map(|i| i.0.clone()).collect();
        if parts.is_empty() {
            return Ty::Unknown;
        }

        // Check if this is a single-segment name that matches a type parameter
        if parts.len() == 1 {
            if self.type_params.contains_key(&parts[0]) {
                // Type parameter - return as Named type with single segment
                // This allows it to be tracked and later substituted during monomorphization
                return Ty::Named(parts);
            }
        }

        // Otherwise use the standard mapping
        map_name_to_ty(np)
    }

    /// Allocate a fresh declaration order number for a new local
    fn next_decl_order(&mut self) -> u32 {
        let order = self.decl_order_counter;
        self.decl_order_counter += 1;
        order
    }

    /// Push a loop/switch label onto the label stack
    /// is_loop should be true for while/for loops, false for switch statements
    fn push_label(&mut self, label: String, is_loop: bool) {
        self.label_finally_depth
            .insert(label.clone(), self.finally_depth);
        self.loop_labels.push((label, is_loop));
    }

    /// Pop the most recent label from the stack
    fn pop_label(&mut self) {
        if let Some((label, _)) = self.loop_labels.pop() {
            self.label_finally_depth.remove(&label);
        }
    }

    /// Check if a label exists in the current scope and whether it's a loop
    /// Returns Some((is_loop, finally_depth_when_declared)) if found, None if not found
    fn find_label(&self, label: &str) -> Option<(bool, u32)> {
        for (name, is_loop) in self.loop_labels.iter().rev() {
            if name == label {
                let depth = self.label_finally_depth.get(label).copied().unwrap_or(0);
                return Some((*is_loop, depth));
            }
        }
        None
    }

    /// Check if any loop/switch is in scope (for unlabeled break/continue)
    fn in_loop_or_switch(&self) -> bool {
        !self.loop_labels.is_empty()
    }

    /// Check if a loop is in scope (for unlabeled continue - switch doesn't count)
    fn in_loop(&self) -> bool {
        self.loop_labels.iter().any(|(_, is_loop)| *is_loop)
    }

    /// Check if a type needs drop (has a deinit function or has droppable fields)
    fn needs_drop(&self, ty: &Ty) -> bool {
        self.needs_drop_inner(ty, &mut std::collections::HashSet::new())
    }

    /// Inner implementation of needs_drop with visited set for cycle detection
    fn needs_drop_inner(
        &self,
        ty: &Ty,
        visited: &mut std::collections::HashSet<(String, String)>,
    ) -> bool {
        match ty {
            Ty::Named(path) | Ty::Generic { path, .. } => {
                if path.is_empty() {
                    return false;
                }
                let type_name = path.last().unwrap().clone();
                let pkg = if path.len() > 1 {
                    path[..path.len() - 1].join(".")
                } else {
                    self.current_pkg.clone().unwrap_or_default()
                };

                // Check for explicit deinit
                if self
                    .types_needing_drop
                    .contains_key(&(pkg.clone(), type_name.clone()))
                {
                    return true;
                }

                // Check for droppable fields (with cycle detection)
                let key = (pkg.clone(), type_name.clone());
                if visited.contains(&key) {
                    return false; // Cycle detected, assume no drop needed
                }
                visited.insert(key);

                if let Some(fields) = self.struct_fields.get(&(pkg, type_name)) {
                    for (_, (field_ty, _, _, _)) in fields {
                        let field_ty_as_ty = namepath_to_ty(field_ty);
                        if self.needs_drop_inner(&field_ty_as_ty, visited) {
                            return true;
                        }
                    }
                }
                false
            }
            // Primitives don't need drop
            Ty::Int | Ty::Float | Ty::Bool | Ty::Char | Ty::String | Ty::Bytes | Ty::Void => false,
            // Function types don't need drop
            Ty::Function(_, _) => false,
            // Reference types don't need drop - they just expire when their lifetime ends
            Ty::Ref { .. } => false,
            // Tuple types need drop if any element needs drop
            Ty::Tuple(elems) => elems.iter().any(|e| self.needs_drop_inner(e, visited)),
            // Never type doesn't need drop (code never reaches there)
            Ty::Never => false,
            // Unknown types conservatively don't need drop
            Ty::Unknown => false,
        }
    }

    /// Check if a type has an EXPLICIT deinit function (not just droppable fields).
    /// Used to determine if partial moves should be rejected.
    fn has_explicit_deinit(&self, ty: &Ty) -> bool {
        match ty {
            Ty::Named(path) | Ty::Generic { path, .. } => {
                if path.is_empty() {
                    return false;
                }
                let type_name = path.last().unwrap().clone();
                let pkg = if path.len() > 1 {
                    path[..path.len() - 1].join(".")
                } else {
                    self.current_pkg.clone().unwrap_or_default()
                };
                // Only explicit deinit counts as "has explicit deinit"
                matches!(
                    self.types_needing_drop.get(&(pkg, type_name)),
                    Some(DeinitKind::Explicit(_))
                )
            }
            _ => false,
        }
    }

    /// Get the qualified type name for drop resolution (e.g., "pkg.TypeName")
    /// Only returns a name for types with explicit deinit functions
    fn drop_ty_name(&self, ty: &Ty) -> Option<String> {
        match ty {
            Ty::Named(path) => {
                if path.is_empty() {
                    return None;
                }
                let type_name = path.last().unwrap().clone();
                let pkg = if path.len() > 1 {
                    path[..path.len() - 1].join(".")
                } else {
                    self.current_pkg.clone().unwrap_or_default()
                };
                // Only return a name for types with explicit deinit
                if matches!(
                    self.types_needing_drop
                        .get(&(pkg.clone(), type_name.clone())),
                    Some(DeinitKind::Explicit(_))
                ) {
                    Some(format!("{}.{}", pkg, type_name))
                } else {
                    None
                }
            }
            _ => None,
        }
    }
    fn push(&mut self) {
        self.stack.push(HashMap::new());
        self.excl_borrows.push(Default::default());
        self.shared_borrows.push(HashMap::new());
        self.prov_borrows.push(Default::default());
        self.lifetime_env.push_scope();
    }
    fn pop(&mut self) {
        let _ = self.stack.pop();
        let _ = self.excl_borrows.pop();
        let _ = self.shared_borrows.pop();
        let _ = self.prov_borrows.pop();
        // Note: lifetime_env.pop_scope() returns errors but we handle them separately
        let _ = self.lifetime_env.pop_scope();
    }
    fn declare(&mut self, name: &str, info: LocalInfo) {
        if let Some(cur) = self.stack.last_mut() {
            cur.insert(name.to_string(), info);
        }
        // Also declare in lifetime environment for borrow tracking
        self.lifetime_env.declare_local(name);
    }

    fn declare_param(&mut self, name: &str, info: LocalInfo) {
        if let Some(cur) = self.stack.last_mut() {
            cur.insert(name.to_string(), info);
        }
        // Parameters are declared with longer lifetime (function scope)
        self.lifetime_env.declare_param(name);
        // Parameters are always initialized (they receive values from caller)
        self.init_env.declare_initialized(name.to_string());
    }

    /// Pop scope and report any lifetime errors
    fn pop_with_lifetime_check(&mut self, sf: &SourceFile, reporter: &mut Reporter) {
        let _ = self.stack.pop();
        let _ = self.excl_borrows.pop();
        let _ = self.shared_borrows.pop();
        let _ = self.prov_borrows.pop();
        let errors = self.lifetime_env.pop_scope();
        for err in errors {
            reporter.emit(Diagnostic::error(err.to_message()).with_file(sf.path.clone()));
        }
    }
    fn get(&self, name: &str) -> Option<&LocalInfo> {
        for m in self.stack.iter().rev() {
            if let Some(v) = m.get(name) {
                return Some(v);
            }
        }
        None
    }
    fn get_mut(&mut self, name: &str) -> Option<&mut LocalInfo> {
        for i in (0..self.stack.len()).rev() {
            if self.stack[i].contains_key(name) {
                return self.stack[i].get_mut(name);
            }
        }
        None
    }

    fn all_names(&self) -> HashSet<String> {
        let mut s = HashSet::new();
        for m in &self.stack {
            for k in m.keys() {
                s.insert(k.clone());
            }
        }
        s
    }

    /// Check if we are currently inside an unsafe context (unsafe block or unsafe function)
    fn is_unsafe_context(&self) -> bool {
        self.unsafe_depth > 0
    }

    // ========== Shared Borrow Tracking ==========

    /// Add a shared borrow for a variable. Returns the region ID.
    /// Creates a new region and tracks it in the current scope.
    /// Note: This uses expression-level borrow tracking (Env.shared_borrows) rather than
    /// function-level tracking (LifetimeEnv.active_borrows) because expression borrows
    /// are short-lived and should not persist to function exit.
    fn add_shared_borrow(
        &mut self,
        name: &str,
        _span: Option<crate::compiler::hir::core::Span>,
    ) -> RegionId {
        // Generate a fresh region ID for this borrow
        let region_id = self.next_region_id;
        self.next_region_id += 1;
        let region = RegionId::new(region_id);

        // Track in current scope's shared borrows
        if let Some(scope) = self.shared_borrows.last_mut() {
            scope.entry(name.to_string()).or_default().push(region);
        }
        region
    }

    /// Check if a variable has any active shared borrows across all scopes
    fn has_shared_borrows(&self, name: &str) -> bool {
        for scope in &self.shared_borrows {
            if let Some(regions) = scope.get(name) {
                if !regions.is_empty() {
                    return true;
                }
            }
        }
        false
    }

    /// Check if a variable has an active exclusive borrow across all scopes
    pub(crate) fn has_exclusive_borrow(&self, name: &str) -> bool {
        for scope in &self.excl_borrows {
            if scope.contains(name) {
                return true;
            }
        }
        false
    }

    /// Check for borrow conflicts when attempting a new borrow or move.
    /// Returns an error message if there's a conflict.
    pub(crate) fn check_borrow_conflict(&self, name: &str, action: BorrowAction) -> Option<String> {
        match action {
            BorrowAction::SharedBorrow => {
                // Shared borrow conflicts with exclusive borrow
                if self.has_exclusive_borrow(name) {
                    return Some(format!(
                        "cannot borrow '{}' as shared because it is already exclusively borrowed",
                        name
                    ));
                }
                None
            }
            BorrowAction::ExclusiveBorrow => {
                // Exclusive borrow conflicts with any existing borrow
                if self.has_exclusive_borrow(name) {
                    return Some(format!(
                        "cannot borrow '{}' as exclusive because it is already exclusively borrowed",
                        name
                    ));
                }
                if self.has_shared_borrows(name) {
                    return Some(format!(
                        "cannot borrow '{}' as exclusive because it has active shared borrows",
                        name
                    ));
                }
                None
            }
            BorrowAction::Move => {
                // Move conflicts with any active borrow
                if self.has_exclusive_borrow(name) {
                    return Some(format!(
                        "cannot move '{}' because it is exclusively borrowed",
                        name
                    ));
                }
                if self.has_shared_borrows(name) {
                    return Some(format!(
                        "cannot move '{}' because it has active shared borrows",
                        name
                    ));
                }
                None
            }
        }
    }

    /// Check if mutation is allowed on a variable.
    /// Mutation is allowed if:
    /// 1. The variable has an active exclusive borrow (borrowMut), OR
    /// 2. The variable has no active borrows at all (direct ownership)
    ///
    /// Mutation is NOT allowed if the variable only has shared borrows,
    /// as shared borrows provide read-only access.
    pub(crate) fn check_mutation_allowed(&self, name: &str) -> Result<(), String> {
        // If we have an exclusive borrow, mutation is allowed
        if self.has_exclusive_borrow(name) {
            return Ok(());
        }

        // If we have shared borrows but no exclusive borrow, mutation is forbidden
        if self.has_shared_borrows(name) {
            return Err(format!(
                "cannot mutate '{}' through a shared borrow; use borrowMut() for mutation",
                name
            ));
        }

        // No borrows at all - direct ownership, mutation is allowed
        Ok(())
    }

    // --- Public API for testing ---

    /// Public wrapper for has_shared_borrows (for testing)
    #[cfg(test)]
    pub fn has_shared_borrows_public(&self, name: &str) -> bool {
        self.has_shared_borrows(name)
    }

    /// Public wrapper for add_excl_borrow (for testing)
    #[cfg(test)]
    pub fn add_excl_borrow(&mut self, name: &str) {
        if let Some(scope) = self.excl_borrows.last_mut() {
            scope.insert(name.to_string());
        }
    }

    /// Public wrapper for add_shared_borrow (for testing)
    #[cfg(test)]
    pub fn add_shared_borrow_test(&mut self, name: &str) -> lifetime::RegionId {
        self.add_shared_borrow(name, None)
    }

    /// Public wrapper for clear_shared_borrows_for (for testing)
    #[cfg(test)]
    pub fn clear_shared_borrows_for_test(&mut self, name: &str) {
        self.clear_shared_borrows_for(name)
    }

    /// Public wrapper for get_shared_borrow_names (for testing)
    #[cfg(test)]
    pub fn get_shared_borrow_names_test(&self) -> HashSet<String> {
        self.get_shared_borrow_names()
    }

    /// Clear all shared borrows for a specific variable across all scopes.
    /// Used when the source of a borrow may have become invalid (e.g., moved in try block).
    fn clear_shared_borrows_for(&mut self, name: &str) {
        for scope in &mut self.shared_borrows {
            scope.remove(name);
        }
    }

    /// Get all names that currently have shared borrows
    fn get_shared_borrow_names(&self) -> HashSet<String> {
        let mut names = HashSet::new();
        for scope in &self.shared_borrows {
            for (name, regions) in scope {
                if !regions.is_empty() {
                    names.insert(name.clone());
                }
            }
        }
        names
    }

    /// Capture the move states of all variables for later joining
    fn capture_move_states(&self) -> HashMap<String, MoveState> {
        let mut states = HashMap::new();
        for scope in &self.stack {
            for (name, info) in scope {
                states.insert(name.clone(), info.move_state.clone());
            }
        }
        states
    }

    /// Apply captured move states back to the environment
    fn restore_move_states(&mut self, states: &HashMap<String, MoveState>) {
        for scope in &mut self.stack {
            for (name, info) in scope.iter_mut() {
                if let Some(state) = states.get(name) {
                    info.move_state = state.clone();
                    // Sync legacy flag: ConditionallyMoved is also treated as "moved"
                    // for the purpose of the legacy `moved` flag, since using a
                    // conditionally moved value is also an error
                    info.moved =
                        matches!(state, MoveState::FullyMoved | MoveState::ConditionallyMoved);
                }
            }
        }
    }

    /// Join two move state snapshots, producing the conservative result
    /// Variables moved on either path become conditionally moved
    fn join_move_states(
        before: &HashMap<String, MoveState>,
        branch1: &HashMap<String, MoveState>,
        branch2: &HashMap<String, MoveState>,
    ) -> HashMap<String, MoveState> {
        let mut result = HashMap::new();

        // Collect all variable names from all snapshots
        let all_names: HashSet<_> = before
            .keys()
            .chain(branch1.keys())
            .chain(branch2.keys())
            .collect();

        for name in all_names {
            let state1 = branch1.get(name).cloned().unwrap_or(MoveState::Available);
            let state2 = branch2.get(name).cloned().unwrap_or(MoveState::Available);
            result.insert(name.clone(), state1.join(&state2));
        }

        result
    }

    /// Join move states from one branch with the before state (for if without else)
    fn join_move_states_single_branch(
        before: &HashMap<String, MoveState>,
        branch: &HashMap<String, MoveState>,
    ) -> HashMap<String, MoveState> {
        let mut result = HashMap::new();

        let all_names: HashSet<_> = before.keys().chain(branch.keys()).collect();

        for name in all_names {
            let before_state = before.get(name).cloned().unwrap_or(MoveState::Available);
            let branch_state = branch.get(name).cloned().unwrap_or(MoveState::Available);
            result.insert(name.clone(), before_state.join(&branch_state));
        }

        result
    }
}

fn unify_int_like(a: &Ty, b: &Ty) -> bool {
    match (a, b) {
        (Ty::Int, Ty::Int) => true,
        // Be tolerant with unknowns to avoid spurious errors when member types
        // are not yet inferred (e.g., struct field accesses in MVP).
        (Ty::Unknown, Ty::Int) | (Ty::Int, Ty::Unknown) | (Ty::Unknown, Ty::Unknown) => true,
        _ => false,
    }
}
fn unify_bool(a: &Ty, b: &Ty) -> bool {
    matches!((a, b), (Ty::Bool, Ty::Bool))
}

fn same_type(a: &Ty, b: &Ty) -> bool {
    match (a, b) {
        // Unknown matches anything (for type inference)
        (Ty::Unknown, _) | (_, Ty::Unknown) => true,
        // Never is a subtype of all types (it can be used where any type is expected)
        (Ty::Never, _) | (_, Ty::Never) => true,
        // Function types
        (Ty::Function(ap, ar), Ty::Function(bp, br)) => {
            if ap.len() != bp.len() {
                return false;
            }
            if !same_type(ar, br) {
                return false;
            }
            for (x, y) in ap.iter().zip(bp.iter()) {
                if !same_type(x, y) {
                    return false;
                }
            }
            true
        }
        (Ty::Named(a), Ty::Named(b)) => a == b,
        // Generic types are equal if their paths and all type arguments match
        (Ty::Generic { path: ap, args: aa }, Ty::Generic { path: bp, args: ba }) => {
            if ap != bp || aa.len() != ba.len() {
                return false;
            }
            for (x, y) in aa.iter().zip(ba.iter()) {
                if !same_type(x, y) {
                    return false;
                }
            }
            true
        }
        // Generic and Named types can match if Generic has no args and paths are equal
        (Ty::Generic { path, args }, Ty::Named(other))
        | (Ty::Named(other), Ty::Generic { path, args }) => args.is_empty() && path == other,
        // Tuple types: must have same arity and element types
        (Ty::Tuple(a_elems), Ty::Tuple(b_elems)) => {
            if a_elems.len() != b_elems.len() {
                return false;
            }
            for (x, y) in a_elems.iter().zip(b_elems.iter()) {
                if !same_type(x, y) {
                    return false;
                }
            }
            true
        }
        // Reference types: inner types must match, modes must match, regions compatible
        (
            Ty::Ref {
                inner: a_inner,
                mode: a_mode,
                region: a_region,
            },
            Ty::Ref {
                inner: b_inner,
                mode: b_mode,
                region: b_region,
            },
        ) => {
            if a_mode != b_mode {
                return false;
            }
            if !same_type(a_inner, b_inner) {
                return false;
            }
            // Regions are compatible if: same concrete, one is a var (inference), or both static
            match (a_region, b_region) {
                (Region::Concrete(a), Region::Concrete(b)) => a == b,
                (Region::Static, Region::Static) => true,
                // Region variables unify with anything during inference
                (Region::Var(_), _) | (_, Region::Var(_)) => true,
                // Concrete and static don't match
                _ => false,
            }
        }
        _ => a == b,
    }
}

/// Check if a type is a type parameter (single-segment Named type in the given set)
fn is_type_param_ty(ty: &Ty, type_params: &[String]) -> bool {
    if let Ty::Named(path) = ty {
        if path.len() == 1 && type_params.contains(&path[0]) {
            return true;
        }
    }
    false
}

/// Get the type parameter name if this type is a type parameter
fn get_type_param_name(ty: &Ty, type_params: &[String]) -> Option<String> {
    if let Ty::Named(path) = ty {
        if path.len() == 1 && type_params.contains(&path[0]) {
            return Some(path[0].clone());
        }
    }
    None
}

/// Try to match parameter types with argument types, building a type parameter substitution map.
/// Returns None if there's a mismatch that can't be resolved.
fn match_types_with_inference(
    param_ty: &Ty,
    arg_ty: &Ty,
    type_params: &[String],
    subst: &mut HashMap<String, Ty>,
) -> bool {
    // If argument is Unknown, accept it
    if matches!(arg_ty, Ty::Unknown) {
        return true;
    }

    // If parameter type is a type parameter, bind or check consistency
    if let Some(param_name) = get_type_param_name(param_ty, type_params) {
        if let Some(existing) = subst.get(&param_name) {
            // Already bound - check consistency
            return same_type(existing, arg_ty);
        } else {
            // Bind the type parameter to the argument type
            subst.insert(param_name, arg_ty.clone());
            return true;
        }
    }

    // If parameter type is Unknown, accept anything
    if matches!(param_ty, Ty::Unknown) {
        return true;
    }

    // Otherwise, types must be structurally equal
    same_type(param_ty, arg_ty)
}

/// Substitute type parameters in a type using the substitution map
fn substitute_type_params(ty: &Ty, subst: &HashMap<String, Ty>) -> Ty {
    match ty {
        Ty::Named(path) => {
            if path.len() == 1 {
                if let Some(bound) = subst.get(&path[0]) {
                    return bound.clone();
                }
            }
            ty.clone()
        }
        Ty::Generic { path, args } => {
            // Substitute in type arguments too
            let new_args: Vec<Ty> = args
                .iter()
                .map(|a| substitute_type_params(a, subst))
                .collect();
            Ty::Generic {
                path: path.clone(),
                args: new_args,
            }
        }
        Ty::Function(params, ret) => {
            let new_params: Vec<Ty> = params
                .iter()
                .map(|p| substitute_type_params(p, subst))
                .collect();
            let new_ret = Box::new(substitute_type_params(ret, subst));
            Ty::Function(new_params, new_ret)
        }
        Ty::Tuple(elems) => {
            // Substitute in tuple element types
            let new_elems: Vec<Ty> = elems
                .iter()
                .map(|e| substitute_type_params(e, subst))
                .collect();
            Ty::Tuple(new_elems)
        }
        // Primitives, Never, and Unknown don't change
        _ => ty.clone(),
    }
}

/// Infer type arguments for a generic function call using the constraint solver.
/// Returns a substitution map if successful, or error messages if inference fails.
///
/// This provides more robust type inference with:
/// - Occurs check to prevent infinite types
/// - Better error messages for type mismatches
/// - Full structural unification for nested generics
fn infer_generic_call(
    generics: &[AS::GenericParam],
    param_types: &[Ty],
    arg_types: &[Ty],
    env: &Env,
) -> Result<HashMap<String, Ty>, Vec<constraint::ConstraintError>> {
    use constraint::ConstraintSolver;

    let mut solver = ConstraintSolver::new(generics);

    // Unify each parameter type with the corresponding argument type
    for (pt, at) in param_types.iter().zip(arg_types.iter()) {
        if matches!(at, Ty::Unknown) {
            continue; // Unknown matches anything
        }
        if !solver.unify(pt, at) {
            // Error already recorded in solver
        }
    }

    // Check bound satisfaction
    solver.check_bounds(|ty, bound| {
        // Convert BoundInfo to NamePath for compatibility with satisfies_bound
        let bound_ty = bound.to_ty();
        check_bound_satisfaction(ty, &bound_ty, env)
    });

    if solver.has_errors() {
        Err(solver.take_errors())
    } else {
        Ok(solver.into_substitution())
    }
}

/// Check if a type satisfies a bound (helper for constraint solver integration)
fn check_bound_satisfaction(ty: &Ty, bound_ty: &Ty, env: &Env) -> bool {
    if same_type(ty, bound_ty) {
        return true;
    }

    match ty {
        Ty::Named(path) | Ty::Generic { path, .. } => {
            if path.is_empty() {
                return false;
            }
            let type_name = path.last().unwrap().clone();
            let pkg = if path.len() > 1 {
                path[..path.len() - 1].join(".")
            } else {
                env.current_pkg.clone().unwrap_or_default()
            };

            if let Some(implemented) = env.implements.get(&(pkg, type_name)) {
                for imp in implemented {
                    if same_type(imp, bound_ty) {
                        return true;
                    }
                }
            }
            false
        }
        _ => false,
    }
}

fn qualify_named_with_imports(path: &mut Vec<String>, env: &Env) {
    if path.len() != 1 {
        return;
    }
    let name = path[0].clone();
    if let Some(pkg) = env.imported_modules.get(&name) {
        *path = vec![pkg.clone(), name];
        return;
    }
    // Try star-imported packages; if exactly one provides a module with this name (per known module fns), qualify it.
    let mut found_pkg: Option<String> = None;
    for p in env.star_import_pkgs.iter() {
        // Check if any function exists under (p, name, _)
        let has_any = env
            .module_fns
            .keys()
            .any(|(pkg, module, _fname)| pkg == p && module == &name);
        if has_any {
            if found_pkg.is_some() {
                // ambiguous; give up
                return;
            }
            found_pkg = Some(p.clone());
        }
    }
    if let Some(pkg) = found_pkg {
        *path = vec![pkg, name];
    }
}

// Expand type aliases: resolve Named paths through the alias map (current or star-imported packages)
fn resolve_alias_ty(mut t: Ty, env: &Env) -> Ty {
    let mut guard = 0;
    loop {
        if guard > 16 {
            return t;
        }
        guard += 1;
        match t.clone() {
            Ty::Named(path) => {
                if path.is_empty() {
                    return t;
                }
                let (pkg, name) = if path.len() == 1 {
                    (env.current_pkg.clone().unwrap_or_default(), path[0].clone())
                } else {
                    (
                        path[..path.len() - 1].join("."),
                        path.last().unwrap().clone(),
                    )
                };
                if let Some(np) = env.type_aliases.get(&(pkg.clone(), name.clone())).cloned() {
                    let mut nt = map_name_to_ty(&np);
                    if let Ty::Named(ref mut p2) = nt {
                        qualify_named_with_imports(p2, env);
                    }
                    t = nt;
                    continue;
                }
                if path.len() == 1 {
                    for sp in env.star_import_pkgs.iter() {
                        if let Some(np) = env.type_aliases.get(&(sp.clone(), name.clone())).cloned()
                        {
                            let mut nt = map_name_to_ty(&np);
                            if let Ty::Named(ref mut p2) = nt {
                                qualify_named_with_imports(p2, env);
                            }
                            t = nt;
                            continue;
                        }
                    }
                }
                return t;
            }
            _ => return t,
        }
    }
}

/// Infer return type from a lambda body by analyzing return statements and last expression
fn infer_lambda_return_type(body: &Block) -> Ty {
    // Helper to find return types from statements
    fn find_return_type_in_stmt(stmt: &Stmt) -> Option<Ty> {
        match stmt {
            Stmt::Return(Some(e)) => Some(infer_expr_type_simple(e)),
            Stmt::Return(None) => Some(Ty::Void),
            Stmt::Block(b) => find_return_type_in_block(b),
            Stmt::If {
                then_blk, else_blk, ..
            } => find_return_type_in_block(then_blk)
                .or_else(|| else_blk.as_ref().and_then(|b| find_return_type_in_block(b))),
            _ => None,
        }
    }

    fn find_return_type_in_block(block: &Block) -> Option<Ty> {
        for stmt in &block.stmts {
            if let Some(ty) = find_return_type_in_stmt(stmt) {
                return Some(ty);
            }
        }
        None
    }

    // First try to find explicit return statement
    if let Some(ty) = find_return_type_in_block(body) {
        return ty;
    }

    // Otherwise check if last statement is an expression
    if let Some(last_stmt) = body.stmts.last() {
        if let Stmt::Expr(e) = last_stmt {
            return infer_expr_type_simple(e);
        }
    }

    // Default to Void
    Ty::Void
}

/// Simple type inference for expressions (without full environment context)
fn infer_expr_type_simple(e: &Expr) -> Ty {
    match e {
        Expr::Int(_) => Ty::Int,
        Expr::Float(_) => Ty::Float,
        Expr::Str(_) => Ty::String,
        Expr::Char(_) => Ty::Char,
        Expr::Bool(_) => Ty::Bool,
        Expr::Binary(_, op, _) => {
            use AS::BinOp;
            match op {
                BinOp::Add
                | BinOp::Sub
                | BinOp::Mul
                | BinOp::Div
                | BinOp::Mod
                | BinOp::BitAnd
                | BinOp::BitOr
                | BinOp::BitXor
                | BinOp::Shl
                | BinOp::Shr => Ty::Int,
                BinOp::Eq
                | BinOp::Ne
                | BinOp::Lt
                | BinOp::Le
                | BinOp::Gt
                | BinOp::Ge
                | BinOp::And
                | BinOp::Or => Ty::Bool,
            }
        }
        Expr::Unary(_, _) => Ty::Unknown,
        Expr::Ternary(_, then_e, _) => infer_expr_type_simple(then_e),
        Expr::Call(_, _) => Ty::Unknown,
        Expr::FnLiteral(_, _) => Ty::Unknown,
        _ => Ty::Unknown,
    }
}

fn type_of_expr(e: &Expr, env: &Env, _sf: &SourceFile, reporter: &mut Reporter) -> Ty {
    match e {
        Expr::FnLiteral(params, body) => {
            // Extract and validate parameter types
            let mut pts: Vec<Ty> = Vec::new();
            for p in params {
                let mut t = map_name_to_ty(&p.ty);
                if let Ty::Named(ref mut pth) = t {
                    qualify_named_with_imports(pth, env);
                }
                t = resolve_alias_ty(t, env);
                pts.push(t);
            }

            // Validate captured variables exist in the current environment
            let captures = collect_lambda_captures(body, params);
            for cap_name in &captures {
                if env.get(cap_name).is_none() {
                    reporter.emit(
                        Diagnostic::error(format!(
                            "captured variable '{}' not found in enclosing scope",
                            cap_name
                        ))
                        .with_file(_sf.path.clone()),
                    );
                }
            }

            // Infer return type from lambda body
            let ret_ty = infer_lambda_return_type(body);

            Ty::Function(pts, Box::new(ret_ty))
        }
        Expr::Int(_) => Ty::Int,
        Expr::Float(_) => Ty::Float,
        Expr::Str(_) => Ty::String,
        Expr::Char(_) => Ty::Char,
        Expr::Bool(_) => Ty::Bool,

        Expr::Await(inner) => {
            // Allow await in async functions or in main() (implicit async entry point)
            if !env.in_async && !env.in_main {
                reporter.emit(
                    Diagnostic::error("'await' is only allowed inside async functions")
                        .with_file(_sf.path.clone()),
                );
            }
            let it = type_of_expr(inner, env, _sf, reporter);
            match it {
                Ty::Unknown => Ty::Unknown,
                // Handle Generic types: Task<T> or Receiver<T> - unwrap and return T
                Ty::Generic { ref path, ref args } => {
                    let last = path.last().map(|s| s.as_str()).unwrap_or("");
                    if last == "Task" || last == "Receiver" {
                        // Unwrap the type argument - return the inner T
                        if let Some(inner_ty) = args.first() {
                            inner_ty.clone()
                        } else {
                            // Generic without type argument - return Unknown
                            Ty::Unknown
                        }
                    } else {
                        let tname = path.join(".");
                        reporter.emit(
                            Diagnostic::error(format!(
                                "expression of type '{}' is not awaitable; expected Task<_> or Receiver<_>",
                                tname
                            ))
                            .with_file(_sf.path.clone()),
                        );
                        Ty::Unknown
                    }
                }
                // Also support Named types for backwards compatibility (bare Task/Receiver)
                Ty::Named(ref path) => {
                    let awaitable = path
                        .last()
                        .map(|s| s.as_str())
                        .is_some_and(|last| last == "Task" || last == "Receiver");
                    if !awaitable {
                        let tname = path.join(".");
                        reporter.emit(
                            Diagnostic::error(format!(
                                "expression of type '{}' is not awaitable; expected Task<_> or Receiver<_>",
                                tname
                            ))
                            .with_file(_sf.path.clone()),
                        );
                    }
                    // Named type without generic args - return Unknown as we can't infer the inner type
                    Ty::Unknown
                }
                _ => {
                    reporter.emit(
                        Diagnostic::error(
                            "expression is not awaitable; expected Task<_> or Receiver<_>",
                        )
                        .with_file(_sf.path.clone()),
                    );
                    Ty::Unknown
                }
            }
        }
        Expr::Cast(tnp, inner) => {
            // Explicit cast: support casts between Int/Float/Bool/Char families.
            let target = map_name_to_ty(tnp);
            let src_t = type_of_expr(inner, env, _sf, reporter);
            match (&target, &src_t) {
                // Identity
                (Ty::Int, Ty::Int)
                | (Ty::Float, Ty::Float)
                | (Ty::Bool, Ty::Bool)
                | (Ty::Char, Ty::Char) => target,
                // Enum -> Int by tag
                (Ty::Int, Ty::Named(path)) if is_enum_named(env, path) => Ty::Int,
                // Numeric family
                (Ty::Float, Ty::Int) | (Ty::Int, Ty::Float) => target,
                // Bool conversions
                (Ty::Bool, Ty::Int)
                | (Ty::Bool, Ty::Float)
                | (Ty::Int, Ty::Bool)
                | (Ty::Float, Ty::Bool) => target,
                // Char conversions (via codepoint)
                (Ty::Char, Ty::Int)
                | (Ty::Int, Ty::Char)
                | (Ty::Char, Ty::Bool)
                | (Ty::Bool, Ty::Char)
                | (Ty::Char, Ty::Float)
                | (Ty::Float, Ty::Char) => target,
                // Unknown tolerated
                (Ty::Int, Ty::Unknown)
                | (Ty::Float, Ty::Unknown)
                | (Ty::Bool, Ty::Unknown)
                | (Ty::Char, Ty::Unknown) => target,
                _ => {
                    reporter.emit(
                        Diagnostic::error(format!("invalid cast from {:?} to {:?}", src_t, target))
                            .with_file(_sf.path.clone()),
                    );
                    Ty::Unknown
                }
            }
        }
        Expr::Ident(id) => {
            if id.0 == "null" {
                reporter.emit(Diagnostic::error("null is not allowed; use Optional<T> (e.g., Optional<int>) and check isPresent()/isEmpty() instead").with_file(_sf.path.clone()));
                return Ty::Unknown;
            }
            if let Some(li) = env.get(&id.0) {
                // Use path-sensitive definite assignment analysis
                if !env.init_env.is_definitely_init(&id.0) {
                    reporter.emit(
                        Diagnostic::error(format!(
                            "use of '{}' before definite initialization",
                            id.0
                        ))
                        .with_file(_sf.path.clone()),
                    );
                }
                if li.moved {
                    reporter.emit(
                        Diagnostic::error(format!("use of moved value '{}'", id.0))
                            .with_file(_sf.path.clone()),
                    );
                }
                if is_excl_borrowed(env, &id.0) {
                    reporter.emit(
                        Diagnostic::error(format!(
                            "cannot use '{}' while it is exclusively borrowed (inferred from a previous borrowMut({})); call release({}) before reusing it",
                            id.0, id.0, id.0
                        ))
                        .with_file(_sf.path.clone()),
                    );
                }
                if let Some(provider_name) =
                    env.lifetime_env.check_invalidated_provider_borrow(&id.0)
                {
                    reporter.emit(
                        Diagnostic::error(format!(
                            "cannot use '{}': borrow from provider '{}' was invalidated by provider mutation; reacquire the borrow after mutation",
                            id.0, provider_name
                        ))
                        .with_file(_sf.path.clone()),
                    );
                }
                li.ty.clone()
            } else {
                // Unknown identifier; resolution should have flagged this, but keep tolerant here.
                Ty::Unknown
            }
        }
        Expr::Binary(l, op, r) => {
            use AS::BinOp as B;
            let lt = type_of_expr(l, env, _sf, reporter);
            let rt = type_of_expr(r, env, _sf, reporter);
            // Helpers for literal-aware safe-only numeric conversions with width info
            fn int_lit_of(expr: &Expr) -> Option<i64> {
                match expr {
                    Expr::Int(n) => Some(*n),
                    Expr::Unary(AS::UnOp::Neg, inner) => {
                        if let Expr::Int(n) = &**inner {
                            Some(-*n)
                        } else {
                            None
                        }
                    }
                    _ => None,
                }
            }
            fn float_bits_of_expr(expr: &Expr, env: &Env) -> Option<u16> {
                // infer float bits for literals, casts, and idents with declared type
                match expr {
                    Expr::Float(_) => Some(64),
                    Expr::Cast(tnp, _inner) => numty_from_namepath(tnp).and_then(|nt| {
                        if matches!(nt.kind, NumKind::Float) {
                            Some(nt.bits)
                        } else {
                            None
                        }
                    }),
                    Expr::Ident(id) => env.get(&id.0).and_then(|li| li.num).and_then(|nt| {
                        if matches!(nt.kind, NumKind::Float) {
                            Some(nt.bits)
                        } else {
                            None
                        }
                    }),
                    _ => None,
                }
            }
            fn int_num_of_expr(expr: &Expr, env: &Env) -> Option<NumTy> {
                match expr {
                    Expr::Int(_) => Some(NumTy {
                        kind: NumKind::IntSigned,
                        bits: 32,
                    }),
                    Expr::Unary(AS::UnOp::Neg, inner) => {
                        if let Expr::Int(_) = &**inner {
                            Some(NumTy {
                                kind: NumKind::IntSigned,
                                bits: 32,
                            })
                        } else {
                            None
                        }
                    }
                    Expr::Cast(tnp, _inner) => numty_from_namepath(tnp)
                        .filter(|nt| matches!(nt.kind, NumKind::IntSigned | NumKind::IntUnsigned)),
                    Expr::Ident(id) => env
                        .get(&id.0)
                        .and_then(|li| li.num)
                        .filter(|nt| matches!(nt.kind, NumKind::IntSigned | NumKind::IntUnsigned)),
                    _ => None,
                }
            }
            fn int_fits_float_exact(n: i64, fbits: u16) -> bool {
                let limit: i64 = match fbits {
                    32 => 16_777_216,
                    _ => 9_007_199_254_740_992,
                }; // 2^24, 2^53
                n >= -limit && n <= limit
            }
            fn allow_int_to_float(ibits: u16, fbits: u16) -> bool {
                match fbits {
                    32 => ibits <= 16,
                    64 => ibits <= 32,
                    _ => false,
                }
            }
            match op {
                B::Add => {
                    // String concatenation: String + String → String
                    if matches!((&lt, &rt), (Ty::String, Ty::String)) {
                        return Ty::String;
                    }
                    // Allow String + Unknown or Unknown + String (for error tolerance)
                    if matches!(
                        (&lt, &rt),
                        (Ty::String, Ty::Unknown) | (Ty::Unknown, Ty::String)
                    ) {
                        return Ty::String;
                    }
                    // Otherwise, fall through to numeric arithmetic
                    let is_num = |t: &Ty| matches!(t, Ty::Int | Ty::Float | Ty::Unknown);
                    if !is_num(&lt) || !is_num(&rt) {
                        reporter.emit(
                            Diagnostic::error("arithmetic on non-numeric operands")
                                .with_file(_sf.path.clone()),
                        );
                        return Ty::Unknown;
                    }
                    match (&lt, &rt) {
                        (Ty::Int, Ty::Int) => {
                            let lnt = int_num_of_expr(l, env);
                            let rnt = int_num_of_expr(r, env);
                            if let (Some(ln), Some(rn)) = (lnt, rnt) {
                                let ls = matches!(ln.kind, NumKind::IntSigned);
                                let rs = matches!(rn.kind, NumKind::IntSigned);
                                if ls != rs {
                                    reporter.emit(Diagnostic::error("mixed signed/unsigned arithmetic requires explicit cast").with_file(_sf.path.clone()));
                                }
                            }
                            Ty::Int
                        }
                        (Ty::Float, Ty::Float)
                        | (Ty::Unknown, Ty::Float)
                        | (Ty::Float, Ty::Unknown) => Ty::Float,
                        (Ty::Unknown, Ty::Unknown) => Ty::Unknown,
                        (Ty::Float, Ty::Int) => {
                            let fbits = float_bits_of_expr(l, env).unwrap_or(64);
                            if let Some(nt) = int_num_of_expr(r, env) {
                                if allow_int_to_float(nt.bits, fbits) {
                                    return Ty::Float;
                                }
                            }
                            if let Some(v) = int_lit_of(r) {
                                if int_fits_float_exact(v, fbits) {
                                    Ty::Float
                                } else {
                                    reporter.emit(Diagnostic::error("int→float arithmetic may lose precision; cast explicitly with (Float)expr").with_file(_sf.path.clone()));
                                    Ty::Float
                                }
                            } else {
                                reporter.emit(Diagnostic::error("int→float arithmetic requires explicit cast per safe-only policy").with_file(_sf.path.clone()));
                                Ty::Float
                            }
                        }
                        (Ty::Int, Ty::Float) => {
                            let fbits = float_bits_of_expr(r, env).unwrap_or(64);
                            if let Some(nt) = int_num_of_expr(l, env) {
                                if allow_int_to_float(nt.bits, fbits) {
                                    return Ty::Float;
                                }
                            }
                            if let Some(v) = int_lit_of(l) {
                                if int_fits_float_exact(v, fbits) {
                                    Ty::Float
                                } else {
                                    reporter.emit(Diagnostic::error("int→float arithmetic may lose precision; cast explicitly with (Float)expr").with_file(_sf.path.clone()));
                                    Ty::Float
                                }
                            } else {
                                reporter.emit(Diagnostic::error("int→float arithmetic requires explicit cast per safe-only policy").with_file(_sf.path.clone()));
                                Ty::Float
                            }
                        }
                        _ => Ty::Int,
                    }
                }
                B::Sub | B::Mul | B::Div | B::Mod => {
                    // Numeric arithmetic with safe-only implicit conversions:
                    // - Int op Int → Int
                    // - Float op Float → Float
                    // - Float op Int or Int op Float → allowed only if the Int side is a literal that fits
                    //   exactly in the float format OR the int type is within the allowed width for the float.
                    // - Unknown tolerated to avoid cascaded errors
                    let is_num = |t: &Ty| matches!(t, Ty::Int | Ty::Float | Ty::Unknown);
                    if !is_num(&lt) || !is_num(&rt) {
                        reporter.emit(
                            Diagnostic::error("arithmetic on non-numeric operands")
                                .with_file(_sf.path.clone()),
                        );
                        return Ty::Unknown;
                    }
                    match (&lt, &rt) {
                        (Ty::Int, Ty::Int) => {
                            let lnt = int_num_of_expr(l, env);
                            let rnt = int_num_of_expr(r, env);
                            if let (Some(ln), Some(rn)) = (lnt, rnt) {
                                let ls = matches!(ln.kind, NumKind::IntSigned);
                                let rs = matches!(rn.kind, NumKind::IntSigned);
                                if ls != rs {
                                    reporter.emit(Diagnostic::error("mixed signed/unsigned arithmetic requires explicit cast").with_file(_sf.path.clone()));
                                }
                            }
                            Ty::Int
                        }
                        (Ty::Float, Ty::Float)
                        | (Ty::Unknown, Ty::Float)
                        | (Ty::Float, Ty::Unknown) => Ty::Float,
                        (Ty::Unknown, Ty::Unknown) => Ty::Unknown,
                        (Ty::Float, Ty::Int) => {
                            let fbits = float_bits_of_expr(l, env).unwrap_or(64);
                            if let Some(nt) = int_num_of_expr(r, env) {
                                if allow_int_to_float(nt.bits, fbits) {
                                    return Ty::Float;
                                }
                            }
                            if let Some(v) = int_lit_of(r) {
                                if int_fits_float_exact(v, fbits) {
                                    Ty::Float
                                } else {
                                    reporter.emit(Diagnostic::error("int→float arithmetic may lose precision; cast explicitly with (Float)expr").with_file(_sf.path.clone()));
                                    Ty::Float
                                }
                            } else {
                                reporter.emit(Diagnostic::error("int→float arithmetic requires explicit cast per safe-only policy").with_file(_sf.path.clone()));
                                Ty::Float
                            }
                        }
                        (Ty::Int, Ty::Float) => {
                            let fbits = float_bits_of_expr(r, env).unwrap_or(64);
                            if let Some(nt) = int_num_of_expr(l, env) {
                                if allow_int_to_float(nt.bits, fbits) {
                                    return Ty::Float;
                                }
                            }
                            if let Some(v) = int_lit_of(l) {
                                if int_fits_float_exact(v, fbits) {
                                    Ty::Float
                                } else {
                                    reporter.emit(Diagnostic::error("int→float arithmetic may lose precision; cast explicitly with (Float)expr").with_file(_sf.path.clone()));
                                    Ty::Float
                                }
                            } else {
                                reporter.emit(Diagnostic::error("int→float arithmetic requires explicit cast per safe-only policy").with_file(_sf.path.clone()));
                                Ty::Float
                            }
                        }
                        _ => Ty::Int,
                    }
                }
                B::Shl | B::Shr => {
                    // Shifts: integers only; result int
                    if unify_int_like(&lt, &rt) {
                        Ty::Int
                    } else {
                        reporter.emit(
                            Diagnostic::error("shift operator on non-integer operands")
                                .with_file(_sf.path.clone()),
                        );
                        Ty::Unknown
                    }
                }
                B::Lt | B::Le | B::Gt | B::Ge | B::Eq | B::Ne => {
                    // Safe-only comparisons for numeric types; otherwise types must match
                    let is_num = |t: &Ty| matches!(t, Ty::Int | Ty::Float | Ty::Unknown);
                    if (matches!((&lt, &rt), (Ty::Int, Ty::Int) | (Ty::Float, Ty::Float)))
                        || matches!((&lt, &rt), (Ty::Unknown, _) | (_, Ty::Unknown))
                    {
                        Ty::Bool
                    } else if is_num(&lt) && is_num(&rt) {
                        // Mixed numbers
                        if matches!((&lt, &rt), (Ty::Float, Ty::Int)) {
                            let fbits = float_bits_of_expr(l, env).unwrap_or(64);
                            if let Some(nt) = int_num_of_expr(r, env) {
                                if allow_int_to_float(nt.bits, fbits) {
                                    return Ty::Bool;
                                }
                            }
                            if let Some(v) = int_lit_of(r) {
                                if int_fits_float_exact(v, fbits) {
                                    Ty::Bool
                                } else {
                                    reporter.emit(Diagnostic::error("comparison int→float may lose precision; cast explicitly").with_file(_sf.path.clone()));
                                    Ty::Bool
                                }
                            } else {
                                reporter.emit(Diagnostic::error("comparison between float and int requires explicit cast per safe-only policy").with_file(_sf.path.clone()));
                                Ty::Bool
                            }
                        } else if matches!((&lt, &rt), (Ty::Int, Ty::Float)) {
                            let fbits = float_bits_of_expr(r, env).unwrap_or(64);
                            if let Some(nt) = int_num_of_expr(l, env) {
                                if allow_int_to_float(nt.bits, fbits) {
                                    return Ty::Bool;
                                }
                            }
                            if let Some(v) = int_lit_of(l) {
                                if int_fits_float_exact(v, fbits) {
                                    Ty::Bool
                                } else {
                                    reporter.emit(Diagnostic::error("comparison int→float may lose precision; cast explicitly").with_file(_sf.path.clone()));
                                    Ty::Bool
                                }
                            } else {
                                reporter.emit(Diagnostic::error("comparison between int and float requires explicit cast per safe-only policy").with_file(_sf.path.clone()));
                                Ty::Bool
                            }
                        } else if matches!((&lt, &rt), (Ty::Int, Ty::Int)) {
                            // Mixed signedness across ints is not implicitly comparable unless the literal is nonnegative
                            let lnt = int_num_of_expr(l, env);
                            let rnt = int_num_of_expr(r, env);
                            let mixed_signedness = match (lnt, rnt) {
                                (
                                    Some(NumTy {
                                        kind: NumKind::IntSigned,
                                        ..
                                    }),
                                    Some(NumTy {
                                        kind: NumKind::IntUnsigned,
                                        ..
                                    }),
                                )
                                | (
                                    Some(NumTy {
                                        kind: NumKind::IntUnsigned,
                                        ..
                                    }),
                                    Some(NumTy {
                                        kind: NumKind::IntSigned,
                                        ..
                                    }),
                                ) => true,
                                _ => false,
                            };
                            if mixed_signedness {
                                let lv = int_lit_of(l);
                                let rv = int_lit_of(r);
                                let ok = lv.map(|x| x >= 0).unwrap_or(false)
                                    || rv.map(|x| x >= 0).unwrap_or(false);
                                if !ok {
                                    reporter.emit(Diagnostic::error("comparison between signed and unsigned integers requires explicit cast").with_file(_sf.path.clone()));
                                }
                            }
                            Ty::Bool
                        } else {
                            Ty::Bool
                        }
                    } else {
                        reporter.emit(
                            Diagnostic::error("comparison between mismatched types")
                                .with_file(_sf.path.clone()),
                        );
                        Ty::Bool
                    }
                }
                B::BitAnd | B::BitOr | B::BitXor => {
                    if unify_int_like(&lt, &rt) {
                        Ty::Int
                    } else {
                        reporter.emit(
                            Diagnostic::error("bitwise operator on non-integer operands")
                                .with_file(_sf.path.clone()),
                        );
                        Ty::Unknown
                    }
                }
                B::And | B::Or => {
                    if unify_bool(&lt, &rt) {
                        Ty::Bool
                    } else {
                        reporter.emit(
                            Diagnostic::error("logical operator on non-bools")
                                .with_file(_sf.path.clone()),
                        );
                        Ty::Bool
                    }
                }
            }
        }
        Expr::Unary(op, inner) => {
            use AS::UnOp as U;
            let it = type_of_expr(inner, env, _sf, reporter);
            match op {
                U::Neg => {
                    if matches!(it, Ty::Int | Ty::Float | Ty::Unknown) {
                        it
                    } else {
                        reporter.emit(
                            Diagnostic::error("unary '-' on non-number")
                                .with_file(_sf.path.clone()),
                        );
                        Ty::Unknown
                    }
                }
                U::Not => {
                    if matches!(it, Ty::Bool | Ty::Unknown) {
                        Ty::Bool
                    } else {
                        reporter
                            .emit(Diagnostic::error("'!' on non-bool").with_file(_sf.path.clone()));
                        Ty::Bool
                    }
                }
            }
        }
        Expr::Call(callee, args) => {
            // Special-case Enum helpers for return typing
            let mut path: Vec<String> = Vec::new();
            if collect_callee_path(callee, &mut path) && path.len() >= 2 {
                let recv = &path[path.len() - 2];
                let fname2 = &path[path.len() - 1];
                if recv == "Enum" {
                    match fname2.as_str() {
                        "tag" if args.len() >= 1 => return Ty::Int,
                        "get" if args.len() >= 2 => return Ty::Int,
                        "payloadCount" if args.len() >= 1 => return Ty::Int,
                        _ => {}
                    }
                }
            }

            // Enum constructor return type: EnumName.Variant(args...)
            // Check if callee matches pattern EnumName.Variant
            if let Expr::Member(obj, variant_name) = callee.as_ref() {
                if let Expr::Ident(enum_ident) = obj.as_ref() {
                    let looks_type = enum_ident
                        .0
                        .chars()
                        .next()
                        .map(|c| c.is_ascii_uppercase())
                        .unwrap_or(false);
                    if looks_type {
                        // Try to find this enum variant
                        if let Some((enum_pkg, _enum_name)) =
                            lookup_enum_variant(env, &enum_ident.0, &variant_name.0)
                        {
                            // Validate payload arguments (done in type_check_call)
                            // Return the enum type
                            let enum_path = if enum_pkg.is_empty() {
                                vec![enum_ident.0.clone()]
                            } else {
                                let mut p: Vec<String> =
                                    enum_pkg.split('.').map(|s| s.to_string()).collect();
                                p.push(enum_ident.0.clone());
                                p
                            };
                            return Ty::Named(enum_path);
                        }
                    }
                }
            }

            // Attempt overload resolution based on argument types
            let cands = lookup_callee_sigs(callee, env);
            if cands.is_empty() {
                // If callee is a function-typed value, return its return type
                let ct = type_of_expr(callee, env, _sf, reporter);
                if let Ty::Function(_ps, ret) = ct {
                    return *ret;
                }
                return Ty::Unknown;
            }
            let arg_types: Vec<Ty> = args
                .iter()
                .map(|a| type_of_expr(a, env, _sf, reporter))
                .collect();
            let norm_param = |np: &AS::NamePath| {
                let mut t = map_name_to_ty(np);
                if let Ty::Named(ref mut pth) = t {
                    qualify_named_with_imports(pth, env);
                    // For function parameters declared as `Fn<...>(...)`,
                    // we don't currently encode full function types on
                    // FuncSig params. Normalize these to Unknown so that
                    // calls with function-typed arguments (lambdas, Fn
                    // locals) are accepted without spurious type errors.
                    if pth.last().map(|s| s.as_str()) == Some("Fn") {
                        return Ty::Unknown;
                    }
                }
                resolve_alias_ty(t, env)
            };
            let mut best: Option<AS::FuncSig> = None;
            let mut best_exact = 0usize;
            let mut best_subst: HashMap<String, Ty> = HashMap::new();
            for s in cands
                .into_iter()
                .filter(|s| s.params.len() == arg_types.len())
            {
                // Extract type parameter names from generics
                let type_params: Vec<String> =
                    s.generics.iter().map(|g| g.name.0.clone()).collect();

                let mut exact = 0usize;
                let mut ok = true;
                let mut subst: HashMap<String, Ty> = HashMap::new();

                for (i, p) in s.params.iter().enumerate() {
                    let pt = norm_param(&p.ty);
                    let at = &arg_types[i];
                    if matches!(at, Ty::Unknown) {
                        continue;
                    }

                    // Use type parameter inference if this is a generic function
                    if !type_params.is_empty() {
                        if match_types_with_inference(&pt, at, &type_params, &mut subst) {
                            if !matches!(pt, Ty::Unknown) && !is_type_param_ty(&pt, &type_params) {
                                exact += 1;
                            }
                        } else {
                            ok = false;
                            break;
                        }
                    } else if same_type(&pt, at) {
                        if !matches!(pt, Ty::Unknown) {
                            exact += 1;
                        }
                    } else {
                        ok = false;
                        break;
                    }
                }
                if ok {
                    if exact > best_exact || (best.is_none() && exact == 0) {
                        best_exact = exact;
                        best = Some(s);
                        best_subst = subst;
                    }
                }
            }
            if let Some(sig) = best {
                for g in &sig.generics {
                    if let Some(bound) = &g.bound {
                        if let Some(arg_ty) = best_subst.get(&g.name.0) {
                            if !satisfies_bound(arg_ty, bound, env) {
                                let bound_str = bound
                                    .path
                                    .iter()
                                    .map(|id| id.0.as_str())
                                    .collect::<Vec<_>>()
                                    .join(".");
                                reporter.emit(
                                    Diagnostic::error(format!(
                                        "type argument '{}' for generic parameter '{}' does not satisfy interface bound '{}'",
                                        ty_to_string(arg_ty),
                                        g.name.0,
                                        bound_str
                                    ))
                                    .with_file(_sf.path.clone()),
                                );
                            }
                        }
                    }
                }

                if sig.is_async {
                    // Async functions return Task<T> where T is the declared return type
                    let inner_ty = if let Some(ref ret) = sig.ret {
                        let raw_ty = map_name_to_ty(ret);
                        substitute_type_params(&raw_ty, &best_subst)
                    } else {
                        Ty::Void
                    };
                    Ty::Generic {
                        path: vec!["concurrent".to_string(), "Task".to_string()],
                        args: vec![inner_ty],
                    }
                } else if let Some(ret) = sig.ret {
                    let raw_ty = map_name_to_ty(&ret);
                    substitute_type_params(&raw_ty, &best_subst)
                } else {
                    Ty::Void
                }
            } else {
                Ty::Unknown
            }
        }
        Expr::Member(obj, name) => {
            // Check for enum variant access: EnumName.VariantName
            // This handles unit variants (no arguments) like Status.Running
            if let Expr::Ident(enum_ident) = &**obj {
                let looks_type = enum_ident
                    .0
                    .chars()
                    .next()
                    .map(|c| c.is_ascii_uppercase())
                    .unwrap_or(false);
                // Only check if it looks like a type AND is not a known local variable
                if looks_type && env.get(&enum_ident.0).is_none() {
                    // Try to find this enum variant
                    if let Some((enum_pkg, _enum_name)) =
                        lookup_enum_variant(env, &enum_ident.0, &name.0)
                    {
                        // Check if this is a unit variant (no payloads)
                        if let Some(payloads) =
                            get_enum_variant_payloads(env, &enum_ident.0, &name.0)
                        {
                            if !payloads.is_empty() {
                                // Data-carrying variant accessed without call - error
                                reporter.emit(
                                    Diagnostic::error(format!(
                                        "enum variant '{}.{}' expects {} argument(s); use '{}.{}(...)' to construct",
                                        enum_ident.0, name.0, payloads.len(), enum_ident.0, name.0
                                    ))
                                    .with_file(_sf.path.clone()),
                                );
                            }
                        }
                        // Return the enum type
                        let enum_path = if enum_pkg.is_empty() {
                            vec![enum_ident.0.clone()]
                        } else {
                            let mut p: Vec<String> =
                                enum_pkg.split('.').map(|s| s.to_string()).collect();
                            p.push(enum_ident.0.clone());
                            p
                        };
                        return Ty::Named(enum_path);
                    }
                }
            }

            // Prefer looking up object type without triggering init/move diagnostics for simple identifiers
            let named_path_opt: Option<Vec<String>> = if let Expr::Ident(id) = &**obj {
                if is_excl_borrowed(env, &id.0) {
                    reporter.emit(
                        Diagnostic::error(format!(
                            "cannot use '{}' while it is exclusively borrowed (inferred from a previous borrowMut({})); call release({}) before reusing it",
                            id.0, id.0, id.0
                        ))
                        .with_file(_sf.path.clone()),
                    );
                }
                // Note: Shared borrow tracking for field access is handled in check_field_access_borrow
                // which is called from statement checking where we have mutable Env access.
                env.get(&id.0).and_then(|li| match &li.ty {
                    Ty::Named(p) => Some(p.clone()),
                    _ => None,
                })
            } else {
                match type_of_expr(obj, env, _sf, reporter) {
                    Ty::Named(p) => Some(p),
                    _ => None,
                }
            };
            match named_path_opt {
                Some(path) => {
                    // Determine (pkg, struct name)
                    if path.is_empty() {
                        return Ty::Unknown;
                    }
                    let (pkg, sname) = if path.len() == 1 {
                        (
                            env.current_pkg.clone().unwrap_or_else(|| String::new()),
                            path[0].clone(),
                        )
                    } else {
                        (
                            path[..path.len() - 1].join("."),
                            path.last().unwrap().clone(),
                        )
                    };
                    if let Some(fields) = env.struct_fields.get(&(pkg.clone(), sname.clone())) {
                        if let Some((fty_np, field_vis, _is_final, _default)) = fields.get(&name.0)
                        {
                            // Check field visibility
                            let current_pkg = env.current_pkg.clone().unwrap_or_default();
                            let can_access = match field_vis {
                                AS::Visibility::Public => true,
                                AS::Visibility::Internal => {
                                    // Internal: accessible within same top-level package
                                    fn top_package_segment(p: &str) -> &str {
                                        p.split('.').next().unwrap_or(p)
                                    }
                                    top_package_segment(&pkg) == top_package_segment(&current_pkg)
                                }
                                AS::Visibility::Private | AS::Visibility::Default => {
                                    // Private/Default: accessible only within same package
                                    pkg == current_pkg
                                }
                            };

                            if !can_access {
                                let vis_str = match field_vis {
                                    AS::Visibility::Public => "public",
                                    AS::Visibility::Internal => "internal",
                                    AS::Visibility::Private => "private",
                                    AS::Visibility::Default => "private",
                                };
                                reporter.emit(
                                    Diagnostic::error(format!(
                                        "field '{}' of struct {} is {} and not accessible from package {}",
                                        name.0, sname, vis_str, current_pkg
                                    ))
                                    .with_file(_sf.path.clone()),
                                );
                                return Ty::Unknown;
                            }

                            let mut ty = map_name_to_ty(fty_np);
                            if let Ty::Named(ref mut pth) = ty {
                                qualify_named_with_imports(pth, env);
                            }
                            return resolve_alias_ty(ty, env);
                        } else {
                            reporter.emit(
                                Diagnostic::error(format!(
                                    "unknown field '{}' in struct {}",
                                    name.0, sname
                                ))
                                .with_file(_sf.path.clone()),
                            );
                            return Ty::Unknown;
                        }
                    }
                    // Not a known struct – leave unknown to avoid false positives for providers/interfaces for now
                    Ty::Unknown
                }
                None => Ty::Unknown,
            }
        }
        Expr::Index(arr, idx) => {
            // Index expression: arr[idx]
            let arr_ty = type_of_expr(arr, env, _sf, reporter);
            let idx_ty = type_of_expr(idx, env, _sf, reporter);

            // Index must be an integer
            if !matches!(idx_ty, Ty::Int | Ty::Unknown) {
                reporter.emit(
                    Diagnostic::error(format!("index must be Int, found '{}'", idx_ty))
                        .with_file(_sf.path.clone()),
                );
            }

            // Determine result type based on container type
            if let Some(elem_ty) = unwrap_list_ty(&arr_ty) {
                // List<T>[Int] -> T
                elem_ty.clone()
            } else if let Some((_key_ty, val_ty)) = unwrap_map_ty(&arr_ty) {
                // Map<K, V>[K] -> Optional<V> (may not exist)
                wrap_in_optional(val_ty.clone())
            } else if matches!(arr_ty, Ty::Unknown) {
                Ty::Unknown
            } else {
                reporter.emit(
                    Diagnostic::error(format!("type '{}' is not indexable", arr_ty))
                        .with_file(_sf.path.clone()),
                );
                Ty::Unknown
            }
        }
        // Optional chaining: obj?.field - returns Optional<FieldType>
        // If obj is Optional<T>, extracts T and accesses field, wrapping result in Optional
        // If obj is not Optional, this is an error
        Expr::OptionalMember(obj, field) => {
            let obj_ty = type_of_expr(obj, env, _sf, reporter);

            // Check if obj is Optional<T>
            if let Some(inner_ty) = unwrap_optional_ty(&obj_ty) {
                // Get the field type from the inner type.
                // When struct metadata is unavailable at this point, keep Optional<Unknown>.
                match inner_ty {
                    Ty::Named(path) | Ty::Generic { path, .. } => {
                        // Try to look up the field type from struct metadata
                        if let Some(field_meta) = lookup_struct_field(env, path, &field.0) {
                            let field_ty = map_name_to_ty(&field_meta.0);
                            wrap_in_optional(field_ty)
                        } else {
                            // Field not found - could be a method or unknown struct
                            wrap_in_optional(Ty::Unknown)
                        }
                    }
                    _ => wrap_in_optional(Ty::Unknown),
                }
            } else if matches!(obj_ty, Ty::Unknown) {
                Ty::Unknown
            } else {
                // Not an Optional type - optional chaining is not valid
                reporter.emit(
                    Diagnostic::error(format!(
                        "optional chaining (?.) requires Optional<T> type, found '{}'",
                        obj_ty
                    ))
                    .with_file(_sf.path.clone()),
                );
                Ty::Unknown
            }
        }
        Expr::Ternary(cond, then_e, else_e) => {
            let ct = type_of_expr(cond, env, _sf, reporter);
            if !matches!(ct, Ty::Bool | Ty::Unknown) {
                reporter.emit(
                    Diagnostic::error("ternary condition must be bool").with_file(_sf.path.clone()),
                );
            }
            let tt = type_of_expr(then_e, env, _sf, reporter);
            let et = type_of_expr(else_e, env, _sf, reporter);
            if same_type(&tt, &et) {
                tt
            } else {
                reporter.emit(
                    Diagnostic::error("ternary branches have mismatched types")
                        .with_file(_sf.path.clone()),
                );
                Ty::Unknown
            }
        }
        Expr::ListLit(elements) => {
            // Infer element type from first element, or Unknown for empty list
            if elements.is_empty() {
                // Empty list - return List<Unknown> (type inference needed from context)
                Ty::Generic {
                    path: vec!["List".to_string()],
                    args: vec![Ty::Unknown],
                }
            } else {
                let elem_ty = type_of_expr(&elements[0], env, _sf, reporter);
                // Check all elements have the same type
                for elem in &elements[1..] {
                    let t = type_of_expr(elem, env, _sf, reporter);
                    if !same_type(&elem_ty, &t) {
                        reporter.emit(
                            Diagnostic::error(format!(
                                "list element type mismatch: expected '{}', found '{}'",
                                elem_ty, t
                            ))
                            .with_file(_sf.path.clone()),
                        );
                        return Ty::Unknown;
                    }
                }
                // Return List<T> where T is the element type
                Ty::Generic {
                    path: vec!["List".to_string()],
                    args: vec![elem_ty],
                }
            }
        }
        Expr::MapLit { pairs, spread } => {
            // If spread is present, check its type
            if let Some(spread_expr) = spread {
                let _spread_ty = type_of_expr(spread_expr, env, _sf, reporter);
                // Spread type validation happens during struct literal checking
            }

            // Infer key and value types from first pair, or Unknown for empty map
            if pairs.is_empty() {
                // Empty map - return Map<Unknown, Unknown> (type inference needed from context)
                Ty::Generic {
                    path: vec!["Map".to_string()],
                    args: vec![Ty::Unknown, Ty::Unknown],
                }
            } else {
                let (first_key, first_val) = &pairs[0];
                let key_ty = type_of_expr(first_key, env, _sf, reporter);
                let val_ty = type_of_expr(first_val, env, _sf, reporter);

                // Check all pairs have the same key and value types
                for (key, value) in &pairs[1..] {
                    let kt = type_of_expr(key, env, _sf, reporter);
                    let vt = type_of_expr(value, env, _sf, reporter);
                    if !same_type(&key_ty, &kt) {
                        reporter.emit(
                            Diagnostic::error(format!(
                                "map key type mismatch: expected '{}', found '{}'",
                                key_ty, kt
                            ))
                            .with_file(_sf.path.clone()),
                        );
                        return Ty::Unknown;
                    }
                    if !same_type(&val_ty, &vt) {
                        reporter.emit(
                            Diagnostic::error(format!(
                                "map value type mismatch: expected '{}', found '{}'",
                                val_ty, vt
                            ))
                            .with_file(_sf.path.clone()),
                        );
                        return Ty::Unknown;
                    }
                }

                // Return Map<K, V> where K is key type, V is value type
                Ty::Generic {
                    path: vec!["Map".to_string()],
                    args: vec![key_ty, val_ty],
                }
            }
        }
        Expr::StructLit {
            type_name,
            fields,
            spread,
        } => {
            // Extract the struct type name
            let struct_path = expr_to_type_path(type_name);

            // Determine the (package, struct name) for this type
            let (pkg, sname) = if struct_path.len() == 1 {
                (
                    env.current_pkg.clone().unwrap_or_default(),
                    struct_path[0].clone(),
                )
            } else {
                (
                    struct_path[..struct_path.len() - 1].join("."),
                    struct_path.last().unwrap().clone(),
                )
            };

            // Check if this is a known struct
            if let Some(struct_fields) = env.struct_fields.get(&(pkg.clone(), sname.clone())) {
                let has_spread = spread.is_some();

                // Validate spread expression if present
                if let Some(spread_expr) = spread {
                    let spread_ty = type_of_expr(spread_expr, env, _sf, reporter);
                    let expected_ty = Ty::Named(struct_path.clone());
                    if !same_type(&expected_ty, &spread_ty) {
                        reporter.emit(
                            Diagnostic::error(format!(
                                "spread expression type mismatch: expected {}, found {:?}",
                                struct_path.join("."),
                                spread_ty
                            ))
                            .with_file(_sf.path.clone()),
                        );
                    }
                }

                // Track provided fields for duplicate and missing field detection
                let mut provided_fields: std::collections::HashSet<String> =
                    std::collections::HashSet::new();

                // Type check each field
                for (field_name, value) in fields {
                    let field_name_str = &field_name.0;

                    // Check for duplicate fields
                    if provided_fields.contains(field_name_str) {
                        reporter.emit(
                            Diagnostic::error(format!(
                                "duplicate field '{}' in struct literal {}",
                                field_name_str, sname
                            ))
                            .with_file(_sf.path.clone()),
                        );
                    } else {
                        provided_fields.insert(field_name_str.clone());
                    }

                    // Check if field exists in struct definition
                    if let Some((field_ty_np, _vis, _is_final, _default)) =
                        struct_fields.get(field_name_str)
                    {
                        // Type check the value
                        let value_ty = type_of_expr(value, env, _sf, reporter);
                        let mut expected_ty = map_name_to_ty(field_ty_np);
                        if let Ty::Named(ref mut pth) = expected_ty {
                            qualify_named_with_imports(pth, env);
                        }
                        expected_ty = resolve_alias_ty(expected_ty, env);

                        if !same_type(&expected_ty, &value_ty) {
                            reporter.emit(
                                Diagnostic::error(format!(
                                    "type mismatch for field '{}' in struct {}: expected {:?}, found {:?}",
                                    field_name_str, sname, expected_ty, value_ty
                                ))
                                .with_file(_sf.path.clone()),
                            );
                        }
                    } else {
                        reporter.emit(
                            Diagnostic::error(format!(
                                "unknown field '{}' in struct {}",
                                field_name_str, sname
                            ))
                            .with_file(_sf.path.clone()),
                        );
                    }
                }

                // Check for missing required fields (only if no spread)
                if !has_spread {
                    for (field_name, (_, _, _, default)) in struct_fields.iter() {
                        // Field is required if it has no default value
                        if default.is_none() && !provided_fields.contains(field_name) {
                            reporter.emit(
                                Diagnostic::error(format!(
                                    "missing required field '{}' in struct literal {}",
                                    field_name, sname
                                ))
                                .with_file(_sf.path.clone()),
                            );
                        }
                    }
                }
            } else {
                // Unknown struct type
                reporter.emit(
                    Diagnostic::error(format!("unknown struct type '{}'", struct_path.join(".")))
                        .with_file(_sf.path.clone()),
                );

                // Still type check the field values
                for (_, value) in fields {
                    let _ = type_of_expr(value, env, _sf, reporter);
                }
            }

            // Return the struct type
            Ty::Named(struct_path)
        }
    }
}

/// Convert an expression (identifier or member chain) to a type path
fn expr_to_type_path(e: &Expr) -> Vec<String> {
    match e {
        Expr::Ident(AS::Ident(name)) => vec![name.clone()],
        Expr::Member(obj, AS::Ident(member)) => {
            let mut path = expr_to_type_path(obj);
            path.push(member.clone());
            path
        }
        _ => vec!["<unknown>".to_string()],
    }
}

/// Check if a type is Copy (can be implicitly copied rather than moved).
/// This checks both built-in Copy types (primitives) and user-defined Copy types
/// from the copy_types index.
fn is_copy_ty(t: &Ty, env: &Env) -> bool {
    match t {
        // Primitives and immutable blobs are always Copy
        Ty::Int | Ty::Float | Ty::Bool | Ty::Char | Ty::String | Ty::Bytes => true,
        // Void is Copy (trivially)
        Ty::Void => true,
        // Named types: check if explicitly marked as Copy or auto-derived
        Ty::Named(path) => {
            if path.is_empty() {
                return false;
            }
            let type_name = path.last().unwrap().clone();
            // Owned<T> is never Copy - it represents unique ownership
            if type_name == "Owned" {
                return false;
            }
            // Determine the package for this type
            let pkg = if path.len() > 1 {
                path[..path.len() - 1].join(".")
            } else {
                env.current_pkg.clone().unwrap_or_default()
            };
            // Check if this type is registered as Copy
            env.copy_types.contains_key(&(pkg, type_name))
        }
        // Generic types: check if the base type is Copy
        Ty::Generic { path, .. } => {
            if path.is_empty() {
                return false;
            }
            let type_name = path.last().unwrap().clone();
            // Owned<T> is never Copy
            if type_name == "Owned" {
                return false;
            }
            // Determine the package for this type
            let pkg = if path.len() > 1 {
                path[..path.len() - 1].join(".")
            } else {
                env.current_pkg.clone().unwrap_or_default()
            };
            // Check if this type is registered as Copy
            env.copy_types.contains_key(&(pkg, type_name))
        }
        // Tuple types are Copy if all elements are Copy
        Ty::Tuple(elems) => elems.iter().all(|e| is_copy_ty(e, env)),
        // Function types are not Copy
        Ty::Function(_, _) => false,
        // Reference types are not Copy - they represent borrows with lifetime constraints
        // Moving a reference would duplicate the borrow, violating uniqueness for exclusive borrows
        Ty::Ref { .. } => false,
        // Never type is Copy (trivially - code never reaches there)
        Ty::Never => true,
        // Unknown types are not Copy (conservative)
        Ty::Unknown => false,
    }
}

/// Check if a type is safe to pass across FFI boundaries.
/// FFI-safe types are primitive numeric types that have well-defined C representations.
/// Arth-owned types (String, structs, enums, etc.) cannot be moved to C code.
fn is_ffi_safe_ty(t: &Ty) -> bool {
    match t {
        // Only numeric primitives and bool are FFI-safe
        Ty::Int | Ty::Float | Ty::Bool | Ty::Char => true,
        // Strings and bytes are Arth-managed heap allocations - NOT FFI-safe
        Ty::String | Ty::Bytes => false,
        // Named types (structs, enums, etc.) are Arth-owned - NOT FFI-safe
        // This includes Owned<T>, Shared<T>, List<T>, Map<K,V>, etc.
        Ty::Named(_) => false,
        // Generic types (Task<T>, Receiver<T>, etc.) are Arth-owned - NOT FFI-safe
        Ty::Generic { .. } => false,
        // Tuple types are not FFI-safe
        Ty::Tuple(_) => false,
        // Function types cannot be passed to C
        Ty::Function(_, _) => false,
        // Reference types are internal to Arth's borrow checker - NOT FFI-safe
        Ty::Ref { .. } => false,
        // Void is technically FFI-safe (for return types)
        Ty::Void => true,
        // Never type is FFI-safe (trivially - code never reaches there)
        Ty::Never => true,
        // Unknown types are conservatively unsafe
        Ty::Unknown => false,
    }
}

/// Check if a NamePath represents an FFI-safe type
fn is_ffi_safe_namepath(np: &AS::NamePath) -> bool {
    if np.path.is_empty() {
        return false;
    }
    let ty_name = &np.path.last().unwrap().0;
    // Only primitive type names are FFI-safe
    matches!(
        ty_name.as_str(),
        "int" | "i8" | "i16" | "i32" | "i64" | "float" | "f32" | "f64" | "bool" | "char" | "void"
    )
}

/// Convert a Ty to a human-readable string representation
fn ty_to_string(t: &Ty) -> String {
    // Use the Display implementation
    format!("{}", t)
}

/// Get a human-readable description of why a type is not FFI-safe
fn ffi_unsafe_reason(t: &Ty) -> &'static str {
    match t {
        Ty::String => "String is an Arth-managed heap allocation",
        Ty::Bytes => "bytes is an Arth-managed heap allocation",
        Ty::Named(path) => {
            if let Some(last) = path.last() {
                match last.as_str() {
                    "Owned" => "Owned<T> represents exclusive Arth ownership",
                    "Shared" => "Shared<T> is an Arth-managed reference-counted type",
                    "List" => "List<T> is an Arth-managed collection",
                    "Map" => "Map<K,V> is an Arth-managed collection",
                    _ => "struct/enum types are Arth-owned and cannot be moved to C",
                }
            } else {
                "unknown named type cannot be passed to C"
            }
        }
        Ty::Function(_, _) => "function types cannot be passed to C",
        Ty::Unknown => "type could not be inferred",
        _ => "type is not FFI-safe",
    }
}

fn consume_ident(name: &str, env: &mut Env, sf: &SourceFile, reporter: &mut Reporter) {
    // Check definite initialization first (before mutable borrow)
    if !env.init_env.is_definitely_init(name) {
        reporter.emit(
            Diagnostic::error(format!("move of '{}' before it is initialized", name))
                .with_file(sf.path.clone()),
        );
        return;
    }

    // Check for any active borrows using the lifetime environment.
    // This catches both shared and exclusive borrows that would be invalidated by a move.
    if let Some(err) = env.lifetime_env.check_move(name) {
        reporter.emit(Diagnostic::error(err.to_message()).with_file(sf.path.clone()));
        return;
    }

    // Check for shared borrow conflicts using Env's direct tracking
    if let Some(err_msg) = env.check_borrow_conflict(name, BorrowAction::Move) {
        reporter.emit(Diagnostic::error(err_msg).with_file(sf.path.clone()));
        return;
    }

    // Legacy check for exclusive borrows (for compatibility with borrowMut/release pattern)
    if is_excl_borrowed(env, name) {
        reporter.emit(
            Diagnostic::error(format!(
                "cannot move '{}' while it is exclusively borrowed (inferred from a previous borrowMut({})); call release({}) before moving it",
                name, name, name
            ))
            .with_file(sf.path.clone()),
        );
        return;
    }

    if let Some(li) = env.get_mut(name) {
        // Check both legacy moved flag and new move_state for compatibility
        if li.moved || li.move_state.is_definitely_moved() {
            reporter.emit(
                Diagnostic::error(format!("use of moved value '{}'", name))
                    .with_file(sf.path.clone()),
            );
            return;
        }
        // Check for conditional moves - value may be moved on some paths
        if matches!(li.move_state, MoveState::ConditionallyMoved) {
            reporter.emit(
                Diagnostic::error(format!(
                    "use of possibly moved value '{}' (moved on some control flow paths)",
                    name
                ))
                .with_file(sf.path.clone()),
            );
            return;
        }
        // Check for partial moves - can't move the whole struct if some fields are already moved
        if let MoveState::PartiallyMoved(ref fields) = li.move_state {
            let field_list: Vec<_> = fields.iter().collect();
            reporter.emit(
                Diagnostic::error(format!(
                    "cannot move '{}' because fields {:?} have already been moved",
                    name, field_list
                ))
                .with_file(sf.path.clone()),
            );
            return;
        }
        li.moved = true;
        li.move_state.move_fully();
    } else {
        reporter.emit(
            Diagnostic::error(format!("use of undeclared local '{}'", name))
                .with_file(sf.path.clone()),
        );
    }
}

/// Consume a specific field of a struct variable (partial move)
fn consume_field(
    var_name: &str,
    field_name: &str,
    env: &mut Env,
    sf: &SourceFile,
    reporter: &mut Reporter,
) {
    // Check definite initialization
    if !env.init_env.is_definitely_init(var_name) {
        reporter.emit(
            Diagnostic::error(format!(
                "move of '{}.{}' before '{}' is initialized",
                var_name, field_name, var_name
            ))
            .with_file(sf.path.clone()),
        );
        return;
    }

    // Check for any active borrows on the parent variable using the lifetime environment.
    // Moving a field invalidates borrows of the whole struct.
    if let Some(err) = env.lifetime_env.check_move(var_name) {
        reporter.emit(
            Diagnostic::error(format!(
                "cannot move field '{}.{}': {}",
                var_name,
                field_name,
                err.to_message()
            ))
            .with_file(sf.path.clone()),
        );
        return;
    }

    // Legacy check for exclusive borrows
    if is_excl_borrowed(env, var_name) {
        reporter.emit(
            Diagnostic::error(format!(
                "cannot move '{}.{}' while '{}' is exclusively borrowed",
                var_name, field_name, var_name
            ))
            .with_file(sf.path.clone()),
        );
        return;
    }

    // First, get info using immutable borrow to avoid conflicts
    let (is_moved, has_deinit, is_field_moved, ty_clone, drop_ty_name_clone) =
        if let Some(li) = env.get(var_name) {
            let is_moved = li.moved || li.move_state.is_definitely_moved();
            let is_field_moved = li.move_state.is_field_moved(field_name);
            (
                is_moved,
                env.has_explicit_deinit(&li.ty),
                is_field_moved,
                Some(li.ty.clone()),
                li.drop_ty_name.clone(),
            )
        } else {
            reporter.emit(
                Diagnostic::error(format!("use of undeclared local '{}'", var_name))
                    .with_file(sf.path.clone()),
            );
            return;
        };

    // Check if the whole struct is already moved
    if is_moved {
        reporter.emit(
            Diagnostic::error(format!(
                "cannot move '{}.{}' because '{}' has already been moved",
                var_name, field_name, var_name
            ))
            .with_file(sf.path.clone()),
        );
        return;
    }

    // Check if the struct type has EXPLICIT deinit - partial moves are only blocked
    // for types with explicit deinit functions (because the deinit might access moved fields).
    // Types with droppable fields but no explicit deinit can be partially moved;
    // we'll emit per-field drops for the remaining fields.
    if has_deinit {
        let type_name = drop_ty_name_clone
            .as_ref()
            .map(|s| s.as_str())
            .unwrap_or("this type");
        reporter.emit(
            Diagnostic::error(format!(
                "cannot move field '{}.{}' out of '{}' because {} has a deinit function; \
                 partial moves from types with destructors are not allowed",
                var_name, field_name, var_name, type_name
            ))
            .with_file(sf.path.clone()),
        );
        return;
    }

    // Check if this specific field is already moved
    if is_field_moved {
        reporter.emit(
            Diagnostic::error(format!("use of moved field '{}.{}'", var_name, field_name))
                .with_file(sf.path.clone()),
        );
        return;
    }

    // Now get a mutable reference to mark the field as moved
    if let Some(li) = env.get_mut(var_name) {
        li.move_state.move_field(field_name);
    }
}

/// Check if reading a field is valid (field not moved) and create a shared borrow.
/// Field access implicitly borrows the parent struct for the duration of the access.
/// For provider types, creates a BorrowOrigin::Provider borrow instead of local borrow.
fn check_field_read(
    var_name: &str,
    field_name: &str,
    env: &mut Env,
    sf: &SourceFile,
    reporter: &mut Reporter,
    current_pkg: &str,
    rp: &crate::compiler::resolve::ResolvedProgram,
) {
    // First check for borrow conflicts - creating a shared borrow for field access
    // conflicts with an existing exclusive borrow on the same variable
    if let Some(conflict_msg) = env.check_borrow_conflict(var_name, BorrowAction::SharedBorrow) {
        reporter.emit(
            Diagnostic::error(format!(
                "cannot access '{}.{}': {}",
                var_name, field_name, conflict_msg
            ))
            .with_file(sf.path.clone())
            .with_suggestion(format!(
                "call release({}) before accessing the field",
                var_name
            )),
        );
        return;
    }

    // Get local info for move state checking and provider type detection
    let check_result = env.get(var_name).map(|li| {
        let moved = li.moved || li.move_state.is_definitely_moved();
        let field_moved = li.move_state.is_field_moved(field_name);
        let conditionally_moved = matches!(li.move_state, MoveState::ConditionallyMoved);
        // Extract type path for provider detection
        let type_path = match &li.ty {
            Ty::Named(path) => Some(path.clone()),
            _ => None,
        };
        (moved, field_moved, conditionally_moved, type_path)
    });

    if let Some((moved, field_moved, conditionally_moved, type_path)) = check_result {
        // Check if whole struct is moved
        if moved {
            reporter.emit(
                Diagnostic::error(format!(
                    "cannot access '{}.{}' because '{}' has been moved",
                    var_name, field_name, var_name
                ))
                .with_file(sf.path.clone()),
            );
            return;
        }
        // Check if this specific field is moved
        if field_moved {
            reporter.emit(
                Diagnostic::error(format!("use of moved field '{}.{}'", var_name, field_name))
                    .with_file(sf.path.clone()),
            );
            return;
        }
        // Check for conditional moves
        if conditionally_moved {
            reporter.emit(
                Diagnostic::error(format!(
                    "cannot access '{}.{}' because '{}' may have been moved on some paths",
                    var_name, field_name, var_name
                ))
                .with_file(sf.path.clone()),
            );
            return;
        }

        // All checks passed - create a shared borrow for this field access
        // For provider types, create a BorrowOrigin::Provider borrow
        // For regular structs, create a BorrowOrigin::Local borrow
        let is_provider = type_path.as_ref().is_some_and(|path| {
            let (pkg, _type_name) = if path.len() == 1 {
                (
                    env.current_pkg
                        .clone()
                        .unwrap_or_else(|| current_pkg.to_string()),
                    path[0].clone(),
                )
            } else {
                (
                    path[..path.len() - 1].join("."),
                    path.last().unwrap().clone(),
                )
            };
            matches!(
                crate::compiler::resolve::lookup_symbol_kind(rp, &pkg, path),
                Some(crate::compiler::resolve::ResolvedKind::Provider)
            )
        });

        if is_provider {
            // Provider field access - create provider-origin borrow
            let provider_name = type_path
                .as_ref()
                .and_then(|p| p.last())
                .map(|s| s.as_str())
                .unwrap_or(var_name);
            let _region_id = env.lifetime_env.borrow_from_provider(
                provider_name,
                None,
                lifetime::BorrowMode::Shared,
                None,
            );
        } else {
            // Regular struct field access - create local borrow
            let _region_id = env.add_shared_borrow(var_name, None);
        }
    }
}

/// If `local_name` is a provider-typed local, return the provider type name.
fn provider_type_name_for_local(
    local_name: &str,
    env: &Env,
    current_pkg: &str,
    rp: &crate::compiler::resolve::ResolvedProgram,
) -> Option<String> {
    let path = match env.get(local_name).map(|li| li.ty.clone()) {
        Some(Ty::Named(path)) => path,
        _ => return None,
    };

    let pkg = if path.len() == 1 {
        env.current_pkg
            .clone()
            .unwrap_or_else(|| current_pkg.to_string())
    } else {
        path[..path.len() - 1].join(".")
    };

    if matches!(
        lookup_symbol_kind(rp, &pkg, &path),
        Some(ResolvedKind::Provider)
    ) {
        path.last().cloned()
    } else {
        None
    }
}

/// Track that `holder` now holds a provider-origin borrow if `expr` is a provider field read.
fn maybe_bind_provider_borrow_holder(
    holder: &str,
    expr: &Expr,
    env: &mut Env,
    current_pkg: &str,
    rp: &crate::compiler::resolve::ResolvedProgram,
) {
    if let Expr::Member(obj, _) = expr
        && let Expr::Ident(provider_local) = obj.as_ref()
        && let Some(provider_name) =
            provider_type_name_for_local(&provider_local.0, env, current_pkg, rp)
    {
        let _ = env.lifetime_env.borrow_from_provider(
            &provider_name,
            Some(holder),
            lifetime::BorrowMode::Shared,
            None,
        );
    }
}

// Walk expression to apply move semantics for pass-by-value call args (non-copy).
fn check_moves_in_expr(
    e: &Expr,
    env: &mut Env,
    sf: &SourceFile,
    reporter: &mut Reporter,
    current_pkg: &str,
    rp: &crate::compiler::resolve::ResolvedProgram,
) {
    match e {
        Expr::FnLiteral(params, body) => {
            // Captures move at creation: move non-copy captured values from outer env
            let caps = collect_lambda_captures(body, params);

            // Check that captured variables are definitely initialized
            for name in &caps {
                if env.get(name).is_some() && !env.init_env.is_definitely_init(name) {
                    reporter.emit(
                        Diagnostic::error(format!(
                            "lambda captures variable '{}' which may not be initialized",
                            name
                        ))
                        .with_file(sf.path.clone()),
                    );
                }
            }

            // Move non-copy captured values from outer env
            for name in &caps {
                if let Some(li) = env.get(name) {
                    if !is_copy_ty(&li.ty, env) {
                        consume_ident(name, env, sf, reporter);
                    }
                }
            }

            // Closure capture borrow validation:
            // Check if any captured variable holds a borrow. If so, the closure
            // capturing it would cause the borrow to potentially outlive its source.
            // This is a safety violation that must be reported.
            for name in &caps {
                // Check if this captured variable holds an active borrow
                if let Some(borrow_info) = env.lifetime_env.get_local_borrow_info(name) {
                    // Report error: closure captures a borrowed reference
                    let origin_desc = match &borrow_info.origin {
                        lifetime::BorrowOrigin::Local(src) => format!("'{}'", src),
                        lifetime::BorrowOrigin::Param(src) => format!("parameter '{}'", src),
                        lifetime::BorrowOrigin::Field(obj, field_path) => {
                            format!("field '{}.{}'", obj, field_path.join("."))
                        }
                        lifetime::BorrowOrigin::Provider(prov) => format!("provider '{}'", prov),
                        lifetime::BorrowOrigin::Unknown => "unknown source".to_string(),
                    };
                    let mode_desc = match borrow_info.mode {
                        lifetime::BorrowMode::Shared => "shared",
                        lifetime::BorrowMode::Exclusive => "exclusive",
                    };
                    reporter.emit(
                        Diagnostic::error(format!(
                            "closure captures '{}' which holds a {} borrow of {}; \
                             captured borrows may outlive their source",
                            name, mode_desc, origin_desc
                        ))
                        .with_file(sf.path.clone()),
                    );
                }
            }

            // Escape analysis: mark captured variables as escaping via closure
            // Closures may outlive the function scope, so captured values need RC
            for name in &caps {
                env.lifetime_env.mark_escape_closure(name);
            }
        }
        Expr::Call(callee, args) => {
            // Recurse into callee; does not move by itself.
            check_moves_in_expr(callee, env, sf, reporter, current_pkg, rp);
            // Basic type/arity checking for calls we can resolve
            type_check_call(callee, args, env, sf, reporter);
            // Implicit effect check for shared handles on common mutators
            effect_check_call(callee, args, env, sf, reporter, current_pkg, rp);
            // Concurrency bound checks for spawn/send points
            concurrency_check_call(callee, args, env, sf, reporter);
            let callee_name = name_of_callee(callee);
            let qualified_name = qualified_callee_name(callee);

            // Escape analysis: determine if this call may store arguments
            // Conservative: functions that take ownership of non-copy values may store them
            // Known safe functions that don't store: borrow helpers, print, collection intrinsics, etc.
            let is_safe_non_storing = is_borrowing_call(qualified_name.as_deref())
                || matches!(
                    callee_name.as_deref(),
                    Some("release" | "print" | "println")
                );

            for a in args {
                // Recurse first (nested calls)
                check_moves_in_expr(a, env, sf, reporter, current_pkg, rp);

                // Handle field access as argument (partial move)
                if let Expr::Member(obj, field) = a {
                    if let Expr::Ident(var_id) = &**obj {
                        // Get the field type to check if it's copyable
                        let field_ty = type_of_expr(a, env, sf, reporter);
                        if !is_copy_ty(&field_ty, env) {
                            // borrow-style helpers and collection intrinsics do not move their arguments
                            if !is_borrowing_call(qualified_name.as_deref()) {
                                // This is a partial move - consume just this field
                                consume_field(&var_id.0, &field.0, env, sf, reporter);
                            }
                        }
                    }
                } else if let Expr::Ident(id) = a
                    && let Some(li) = env.get(&id.0)
                    && !is_copy_ty(&li.ty, env)
                {
                    // borrow-style helpers and collection intrinsics do not move their arguments
                    if !is_borrowing_call(qualified_name.as_deref()) {
                        consume_ident(&id.0, env, sf, reporter);
                    }

                    // Escape analysis: if this is a storing call, mark the argument as escaping
                    // A storing call is any call that takes ownership and might store the value
                    // (constructors, setters, etc.). We're conservative here.
                    if !is_safe_non_storing {
                        env.lifetime_env.mark_escape_call(&id.0);
                    }
                }
            }
        }
        Expr::Ternary(c, t, f) => {
            check_moves_in_expr(c, env, sf, reporter, current_pkg, rp);
            check_moves_in_expr(t, env, sf, reporter, current_pkg, rp);
            check_moves_in_expr(f, env, sf, reporter, current_pkg, rp);
        }
        Expr::Cast(_t, inner) => {
            check_moves_in_expr(inner, env, sf, reporter, current_pkg, rp);
        }
        Expr::Await(inner) => {
            // Enforce await boundary rules.
            check_moves_in_expr(inner, env, sf, reporter, current_pkg, rp);
            if let Some(top) = env.excl_borrows.last()
                && !top.is_empty()
            {
                reporter.emit(
                    Diagnostic::error("exclusive borrows must not cross an await boundary")
                        .with_file(sf.path.clone()),
                );
            }
            // Full lifetime-based await boundary analysis with detailed error messages
            // This provides comprehensive tracking of which borrows are live at await points
            let (await_errors, live_borrows) = env.lifetime_env.check_await_boundary_full();

            // Report errors for exclusive borrows (which are forbidden)
            for err in await_errors {
                reporter.emit(Diagnostic::error(err.to_message()).with_file(sf.path.clone()));
            }

            // P4-7: Check that shared borrows across await are of Shareable types
            // Non-Shareable shared borrows cannot safely cross task boundaries
            for borrow_info in &live_borrows {
                if borrow_info.mode == lifetime::BorrowMode::Shared {
                    // Get the type of the borrowed value if we can
                    if let lifetime::BorrowOrigin::Local(ref src_name) = borrow_info.origin {
                        if let Some(li) = env.get(src_name) {
                            if !is_shareable(env, &li.ty) {
                                reporter.emit(
                                    Diagnostic::error(format!(
                                        "shared borrow of '{}' of type '{}' cannot cross await boundary; \
                                         the type is not Shareable",
                                        src_name,
                                        ty_to_string(&li.ty)
                                    ))
                                    .with_file(sf.path.clone()),
                                );
                            }
                        }
                    }
                }
            }

            // If there are multiple live borrows at await, provide detailed diagnostics
            // This helps developers understand what state is held across the await
            if live_borrows.len() > 1 {
                let borrow_descriptions: Vec<String> =
                    live_borrows.iter().map(|b| b.describe()).collect();
                // This is informational - shared borrows across await are allowed but
                // tracking them helps with understanding async function state
                let _ = borrow_descriptions; // Available for future diagnostics/linting
            }
        }
        Expr::Binary(l, _op, r) => {
            check_moves_in_expr(l, env, sf, reporter, current_pkg, rp);
            check_moves_in_expr(r, env, sf, reporter, current_pkg, rp);
        }
        Expr::Unary(_op, inner) => {
            check_moves_in_expr(inner, env, sf, reporter, current_pkg, rp);
        }
        Expr::Member(obj, m) => {
            // Check the object for moves, but handle field access specially for partial moves
            if let Expr::Ident(id) = &**obj {
                // Check if reading this field is valid (not moved)
                check_field_read(&id.0, &m.0, env, sf, reporter, current_pkg, rp);
            } else {
                // For complex expressions, just recurse
                check_moves_in_expr(obj, env, sf, reporter, current_pkg, rp);
            }
        }
        Expr::OptionalMember(obj, m) => {
            // Optional chaining: check the object expression for moves and create shared borrow
            if let Expr::Ident(id) = &**obj {
                check_field_read(&id.0, &m.0, env, sf, reporter, current_pkg, rp);
            } else {
                check_moves_in_expr(obj, env, sf, reporter, current_pkg, rp);
            }
        }
        Expr::Index(obj, idx) => {
            // Index access creates a shared borrow of the container
            if let Expr::Ident(id) = &**obj {
                // Check for borrow conflicts before creating shared borrow
                if let Some(conflict_msg) =
                    env.check_borrow_conflict(&id.0, BorrowAction::SharedBorrow)
                {
                    reporter.emit(
                        Diagnostic::error(format!("cannot index '{}': {}", id.0, conflict_msg))
                            .with_file(sf.path.clone()),
                    );
                } else {
                    // Create shared borrow for index access
                    let _region_id = env.add_shared_borrow(&id.0, None);
                }
            } else {
                check_moves_in_expr(obj, env, sf, reporter, current_pkg, rp);
            }
            check_moves_in_expr(idx, env, sf, reporter, current_pkg, rp);
        }
        _ => {}
    }
}

fn name_of_callee(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Ident(id) => Some(id.0.clone()),
        Expr::Member(_, id) => Some(id.0.clone()),
        _ => None,
    }
}

/// Get the fully qualified callee name for module function calls (e.g., "List.push")
fn qualified_callee_name(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Ident(id) => Some(id.0.clone()),
        Expr::Member(obj, method) => {
            if let Expr::Ident(module) = &**obj {
                Some(format!("{}.{}", module.0, method.0))
            } else {
                Some(method.0.clone())
            }
        }
        _ => None,
    }
}

/// Check if a function call borrows its first argument rather than moving it.
/// Collection intrinsics operate on handles and should not consume the collection variable.
fn is_borrowing_call(qualified_name: Option<&str>) -> bool {
    matches!(
        qualified_name,
        Some(
            // Borrow helpers
            "borrowMut" | "borrowFromProvider" |
            // List intrinsics - all operate on handles, don't consume
            "List.new" | "List.push" | "List.get" | "List.set" | "List.len" |
            "List.remove" | "List.sort" | "List.pop" | "List.indexOf" |
            "List.contains" | "List.insert" | "List.clear" | "List.reverse" |
            "List.concat" | "List.slice" | "List.unique" | "List.find" |
            "List.findIndex" | "List.findLast" | "List.any" | "List.all" |
            "List.partition" | "List.merge" | "List.forEach" | "List.forEachIndexed" |
            // Map intrinsics - all operate on handles, don't consume
            "Map.new" | "Map.put" | "Map.get" | "Map.len" | "Map.containsKey" |
            "Map.remove" | "Map.keys" | "Map.merge" | "Map.containsValue" |
            "Map.clear" | "Map.isEmpty" | "Map.getOrDefault" | "Map.values" |
            "Map.entries" | "Map.toEntries" | "Map.fromEntries" | "Map.update" |
            "Map.forEach" | "Map.any" | "Map.all" | "Map.find" | "Map.findKey" |
            "Map.filter" | "Map.filterKeys" | "Map.mapValues" | "Map.partition" |
            // Enum intrinsics - read-only access
            "Enum.tag" | "Enum.get" | "Enum.payloadCount"
        )
    )
}

// Collect captured identifiers for a lambda body: identifiers used that are not declared inside
// the lambda (including its parameters). This is a conservative analysis and may over-approximate
// in the presence of shadowing, which is acceptable for move-at-creation semantics.
fn collect_lambda_captures(
    body: &Block,
    params: &[AS::Param],
) -> std::collections::HashSet<String> {
    use std::collections::HashSet;
    fn visit_expr(
        e: &Expr,
        locals: &std::collections::HashSet<String>,
        used: &mut HashSet<String>,
    ) {
        match e {
            Expr::Int(_) | Expr::Float(_) | Expr::Str(_) | Expr::Char(_) | Expr::Bool(_) => {}
            Expr::Ident(id) => {
                if !locals.contains(&id.0) {
                    used.insert(id.0.clone());
                }
            }
            Expr::Await(inner) => visit_expr(inner, locals, used),
            Expr::Cast(_t, inner) => visit_expr(inner, locals, used),
            Expr::Ternary(c, t, f) => {
                visit_expr(c, locals, used);
                visit_expr(t, locals, used);
                visit_expr(f, locals, used);
            }
            Expr::Binary(l, _op, r) => {
                visit_expr(l, locals, used);
                visit_expr(r, locals, used);
            }
            Expr::Unary(_op, inner) => visit_expr(inner, locals, used),
            Expr::Call(c, args) => {
                visit_expr(c, locals, used);
                for a in args {
                    visit_expr(a, locals, used);
                }
            }
            Expr::Member(obj, _m) => visit_expr(obj, locals, used),
            Expr::OptionalMember(obj, _m) => visit_expr(obj, locals, used),
            Expr::Index(obj, idx) => {
                visit_expr(obj, locals, used);
                visit_expr(idx, locals, used);
            }
            Expr::ListLit(elements) => {
                for elem in elements {
                    visit_expr(elem, locals, used);
                }
            }
            Expr::MapLit { pairs, spread } => {
                if let Some(spread_expr) = spread {
                    visit_expr(spread_expr, locals, used);
                }
                for (key, value) in pairs {
                    visit_expr(key, locals, used);
                    visit_expr(value, locals, used);
                }
            }
            Expr::StructLit {
                type_name,
                fields,
                spread,
            } => {
                // Visit the type name expression in case it references a captured variable
                visit_expr(type_name, locals, used);
                if let Some(spread_expr) = spread {
                    visit_expr(spread_expr, locals, used);
                }
                for (_field_name, value) in fields {
                    visit_expr(value, locals, used);
                }
            }
            Expr::FnLiteral(_ps, _b) => {
                // Do not traverse into nested lambdas here; their captures are independent
            }
        }
    }
    fn visit_block(
        b: &Block,
        locals: &mut std::collections::HashSet<String>,
        used: &mut HashSet<String>,
    ) {
        for s in &b.stmts {
            match s {
                Stmt::PrintStr(_) => {}
                Stmt::PrintRawStr(_) => {}
                Stmt::PrintExpr(e) => visit_expr(e, locals, used),
                Stmt::PrintRawExpr(e) => visit_expr(e, locals, used),
                Stmt::If {
                    cond,
                    then_blk,
                    else_blk,
                } => {
                    visit_expr(cond, locals, used);
                    visit_block(then_blk, locals, used);
                    if let Some(eb) = else_blk {
                        visit_block(eb, locals, used);
                    }
                }
                Stmt::While { cond, body } => {
                    visit_expr(cond, locals, used);
                    visit_block(body, locals, used);
                }
                Stmt::For {
                    init,
                    cond,
                    step,
                    body,
                } => {
                    if let Some(i) = init {
                        visit_simple_stmt(i, locals, used);
                    }
                    if let Some(c) = cond {
                        visit_expr(c, locals, used);
                    }
                    if let Some(st) = step {
                        visit_simple_stmt(st, locals, used);
                    }
                    visit_block(body, locals, used);
                }
                Stmt::Labeled { stmt, .. } => visit_simple_stmt(stmt, locals, used),
                Stmt::Switch {
                    expr,
                    cases,
                    pattern_cases,
                    default,
                } => {
                    visit_expr(expr, locals, used);
                    for (_e, blk) in cases {
                        visit_block(blk, locals, used);
                    }
                    for (_p, blk) in pattern_cases {
                        visit_block(blk, locals, used);
                    }
                    if let Some(bd) = default {
                        visit_block(bd, locals, used);
                    }
                }
                Stmt::Try {
                    try_blk,
                    catches,
                    finally_blk,
                } => {
                    visit_block(try_blk, locals, used);
                    for cb in catches {
                        visit_block(&cb.blk, locals, used);
                    }
                    if let Some(fb) = finally_blk {
                        visit_block(fb, locals, used);
                    }
                }
                Stmt::Assign { name: _n, expr } => visit_expr(expr, locals, used),
                Stmt::FieldAssign { object, expr, .. } => {
                    visit_expr(object, locals, used);
                    visit_expr(expr, locals, used);
                }
                Stmt::AssignOp { expr, .. } => visit_expr(expr, locals, used),
                Stmt::VarDecl { name, init, .. } => {
                    if let Some(e) = init {
                        visit_expr(e, locals, used);
                    }
                    locals.insert(name.0.clone());
                }
                Stmt::Break(_) | Stmt::Continue(_) => {}
                Stmt::Return(e) => {
                    if let Some(x) = e {
                        visit_expr(x, locals, used)
                    }
                }
                Stmt::Throw(e) => visit_expr(e, locals, used),
                Stmt::Panic(e) => visit_expr(e, locals, used),
                Stmt::Block(b2) => visit_block(b2, locals, used),
                Stmt::Unsafe(b2) => visit_block(b2, locals, used),
                Stmt::Expr(e) => visit_expr(e, locals, used),
            }
        }
    }
    fn visit_simple_stmt(
        s: &Stmt,
        locals: &mut std::collections::HashSet<String>,
        used: &mut HashSet<String>,
    ) {
        match s {
            Stmt::Assign { name: _n, expr } => visit_expr(expr, locals, used),
            Stmt::AssignOp { expr, .. } => visit_expr(expr, locals, used),
            Stmt::VarDecl { name, init, .. } => {
                if let Some(e) = init {
                    visit_expr(e, locals, used);
                }
                locals.insert(name.0.clone());
            }
            Stmt::PrintExpr(e) => visit_expr(e, locals, used),
            Stmt::PrintRawExpr(e) => visit_expr(e, locals, used),
            Stmt::Expr(e) => visit_expr(e, locals, used),
            Stmt::FieldAssign { object, expr, .. } => {
                visit_expr(object, locals, used);
                visit_expr(expr, locals, used);
            }
            Stmt::Block(b) => visit_block(b, locals, used),
            Stmt::If {
                cond,
                then_blk,
                else_blk,
            } => {
                visit_expr(cond, locals, used);
                visit_block(then_blk, locals, used);
                if let Some(eb) = else_blk {
                    visit_block(eb, locals, used);
                }
            }
            Stmt::While { cond, body } => {
                visit_expr(cond, locals, used);
                visit_block(body, locals, used);
            }
            Stmt::For {
                init,
                cond,
                step,
                body,
            } => {
                if let Some(i) = init {
                    visit_simple_stmt(i, locals, used);
                }
                if let Some(c) = cond {
                    visit_expr(c, locals, used);
                }
                if let Some(st) = step {
                    visit_simple_stmt(st, locals, used);
                }
                visit_block(body, locals, used);
            }
            Stmt::Labeled { stmt, .. } => visit_simple_stmt(stmt, locals, used),
            Stmt::Switch {
                expr,
                cases,
                pattern_cases,
                default,
            } => {
                visit_expr(expr, locals, used);
                for (_e, blk) in cases {
                    visit_block(blk, locals, used);
                }
                for (_p, blk) in pattern_cases {
                    visit_block(blk, locals, used);
                }
                if let Some(db) = default {
                    visit_block(db, locals, used);
                }
            }
            Stmt::Try {
                try_blk,
                catches,
                finally_blk,
            } => {
                visit_block(try_blk, locals, used);
                for cb in catches {
                    visit_block(&cb.blk, locals, used);
                }
                if let Some(fb) = finally_blk {
                    visit_block(fb, locals, used);
                }
            }
            Stmt::Unsafe(b2) => visit_block(b2, locals, used),
            Stmt::Throw(e) => visit_expr(e, locals, used),
            Stmt::Panic(e) => visit_expr(e, locals, used),
            Stmt::PrintStr(_)
            | Stmt::PrintRawStr(_)
            | Stmt::Break(_)
            | Stmt::Continue(_)
            | Stmt::Return(_) => {}
        }
    }

    let mut locals: std::collections::HashSet<String> =
        params.iter().map(|p| p.name.0.clone()).collect();
    let mut used: HashSet<String> = HashSet::new();
    visit_block(body, &mut locals, &mut used);
    used
}

fn collect_callee_path(e: &Expr, out: &mut Vec<String>) -> bool {
    match e {
        Expr::Ident(id) => {
            out.push(id.0.clone());
            true
        }
        Expr::Member(obj, name) => {
            if collect_callee_path(obj, out) {
                out.push(name.0.clone());
                true
            } else {
                false
            }
        }
        _ => false,
    }
}

fn lookup_callee_raw(
    callee: &Expr,
    env: &Env,
) -> (Vec<AS::FuncSig>, Option<(String, Option<String>)>) {
    let mut path: Vec<String> = Vec::new();
    if !collect_callee_path(callee, &mut path) {
        return (Vec::new(), None);
    }
    let current_pkg = env.current_pkg.clone().unwrap_or_default();
    if path.len() == 1 {
        if let Some(cm) = &env.current_module {
            if let Some(sigs) = env
                .module_fns
                .get(&(current_pkg.clone(), cm.clone(), path[0].clone()))
                .cloned()
            {
                return (sigs, Some((current_pkg.clone(), Some(cm.clone()))));
            }
        }
        return (
            env.free_fns
                .get(&(current_pkg.clone(), path[0].clone()))
                .cloned()
                .unwrap_or_default(),
            Some((current_pkg, None)),
        );
    }
    // Try module function first. If the path is exactly two segments like
    // 'Module.func', allow resolving 'Module' using imports (e.g., from
    // 'import pkg.*;' or 'import pkg.Module;').
    let func = path.last().unwrap().clone();
    let module = path[path.len() - 2].clone();
    let pkg = if path.len() > 2 {
        path[..path.len() - 2].join(".")
    } else {
        // 2-segment form: prefer imported module package if present, else current package
        if let Some(p) = env.imported_modules.get(&module) {
            p.clone()
        } else {
            current_pkg.clone()
        }
    };
    if let Some(sigs) = env
        .module_fns
        .get(&(pkg.clone(), module.clone(), func.clone()))
    {
        return (sigs.clone(), Some((pkg.clone(), Some(module.clone()))));
    }
    // If not found and this was a 2-segment path, try star-imported packages for the module
    if path.len() == 2 {
        for p in env.star_import_pkgs.iter() {
            if let Some(sigs) = env
                .module_fns
                .get(&(p.clone(), module.clone(), func.clone()))
            {
                return (sigs.clone(), Some((p.clone(), Some(module.clone()))));
            }
        }
        // Static struct method sugar: TypeName.method(...) resolves to
        // companion module <TypeName>Fns.method(...) when:
        //  - there is a struct TypeName in a visible package, and
        //  - the companion module TypeNameFns defines a function `method`.
        //
        // Resolution order:
        //  1) Prefer the package chosen for the 2-segment form above (imported
        //     module package if present, else current package).
        //  2) Fall back to structs found via star-imported packages.
        let mut candidate_pkgs: Vec<String> = Vec::new();
        candidate_pkgs.push(pkg.clone());
        for p in env.star_import_pkgs.iter() {
            if !candidate_pkgs.contains(p) {
                candidate_pkgs.push(p.clone());
            }
        }
        for spkg in candidate_pkgs {
            if env
                .struct_fields
                .contains_key(&(spkg.clone(), module.clone()))
            {
                let companion = format!("{}Fns", module);
                if let Some(sigs) =
                    env.module_fns
                        .get(&(spkg.clone(), companion.clone(), func.clone()))
                {
                    return (sigs.clone(), Some((spkg.clone(), Some(companion))));
                }
            }
        }
    }
    // Fallback to free function with prefix as package
    let pkg2 = if path.len() > 1 {
        path[..path.len() - 1].join(".")
    } else {
        current_pkg
    };
    (
        env.free_fns
            .get(&(pkg2.clone(), func))
            .cloned()
            .unwrap_or_default(),
        Some((pkg2, None)),
    )
}

fn is_visible(
    vis: &AS::Visibility,
    callee_pkg: &str,
    callee_module: Option<&str>,
    env: &Env,
) -> bool {
    match vis {
        AS::Visibility::Public => true,
        AS::Visibility::Internal => env.current_pkg.as_deref() == Some(callee_pkg),
        AS::Visibility::Private | AS::Visibility::Default => {
            env.current_pkg.as_deref() == Some(callee_pkg)
                && env.current_module.as_deref() == callee_module
        }
    }
}

fn lookup_callee_sigs(callee: &Expr, env: &Env) -> Vec<AS::FuncSig> {
    let (raw, key) = lookup_callee_raw(callee, env);
    let mut out: Vec<AS::FuncSig> = Vec::new();
    if let Some((pkg, module_opt)) = key {
        for s in raw.into_iter() {
            if is_visible(&s.vis, &pkg, module_opt.as_deref(), env) {
                out.push(s);
            }
        }
    }
    out
}

fn format_callee_for_error(callee: &Expr, _env: &Env) -> String {
    let mut path: Vec<String> = Vec::new();
    if collect_callee_path(callee, &mut path) {
        return join_path(&path);
    }
    name_of_callee(callee).unwrap_or_else(|| "<call>".to_string())
}

fn type_check_call(
    callee: &Expr,
    args: &[Expr],
    env: &mut Env,
    sf: &SourceFile,
    reporter: &mut Reporter,
) {
    let mut handled_enum_constructor = false;

    // If calling a method like 'x.m(...)', ensure 'x' is a declared local when it looks like a variable (lowercase start)
    if let Expr::Member(obj, _name) = callee {
        if let Expr::Ident(id) = &**obj {
            let looks_like_var =
                id.0.chars()
                    .next()
                    .map(|c| c.is_ascii_lowercase())
                    .unwrap_or(false);
            if looks_like_var && env.get(&id.0).is_none() {
                reporter.emit(
                    Diagnostic::error(format!("use of undeclared local '{}'", id.0))
                        .with_file(sf.path.clone()),
                );
            }
        }
    }
    // Enforce generics for List/Map before signature resolution (works for member sugar where we can't resolve a sig)
    // Member sugar: xs.push(v) or m.put(k,v)
    if let Expr::Member(obj, mname) = callee {
        // Validate enum constructors: EnumName.Variant(args...)
        if let Expr::Ident(enum_ident) = &**obj {
            let looks_type = enum_ident
                .0
                .chars()
                .next()
                .map(|c| c.is_ascii_uppercase())
                .unwrap_or(false);
            if looks_type && env.get(&enum_ident.0).is_none() {
                // Use cross-package lookup
                if let Some(payloads) = get_enum_variant_payloads(env, &enum_ident.0, &mname.0) {
                    handled_enum_constructor = true;
                    // Validate argument count
                    if args.len() != payloads.len() {
                        if payloads.is_empty() {
                            // Unit variant called with args
                            reporter.emit(
                                Diagnostic::error(format!(
                                    "enum variant '{}.{}' is a unit variant and takes no arguments; use '{}.{}' instead",
                                    enum_ident.0, mname.0, enum_ident.0, mname.0
                                ))
                                .with_file(sf.path.clone()),
                            );
                        } else {
                            reporter.emit(
                                Diagnostic::error(format!(
                                    "enum constructor '{}.{}' expects {} argument(s), found {}",
                                    enum_ident.0,
                                    mname.0,
                                    payloads.len(),
                                    args.len()
                                ))
                                .with_file(sf.path.clone()),
                            );
                        }
                    } else {
                        // Validate payload types
                        for (i, (pty, a)) in payloads.iter().zip(args.iter()).enumerate() {
                            let mut ety = map_name_to_ty(pty);
                            if let Ty::Named(ref mut pth) = ety {
                                qualify_named_with_imports(pth, env);
                            }
                            ety = resolve_alias_ty(ety, env);
                            let aty = type_of_expr(a, env, sf, reporter);
                            if !same_type(&ety, &aty) {
                                reporter.emit(
                                    Diagnostic::error(format!(
                                        "enum payload {} type mismatch: expected {:?}, found {:?}",
                                        i, ety, aty
                                    ))
                                    .with_file(sf.path.clone()),
                                );
                            }
                        }
                    }
                } else {
                    // Check if this looks like an enum (has any variants) but variant doesn't exist
                    let is_known_enum =
                        env.enum_variants.keys().any(|(_, e, _)| e == &enum_ident.0);
                    if is_known_enum {
                        handled_enum_constructor = true;
                        // Collect known variants for helpful error message
                        let known_variants: Vec<String> = env
                            .enum_variants
                            .keys()
                            .filter(|(_, e, _)| e == &enum_ident.0)
                            .map(|(_, _, v)| v.clone())
                            .collect();
                        reporter.emit(
                            Diagnostic::error(format!(
                                "unknown variant '{}' for enum '{}'; known variants: {}",
                                mname.0,
                                enum_ident.0,
                                known_variants.join(", ")
                            ))
                            .with_file(sf.path.clone()),
                        );
                    }
                }
            }
        }
        if mname.0 == "push" && args.len() >= 1 {
            if let Expr::Ident(id) = &**obj {
                if let Some(li) = env.get(&id.0) {
                    if let Some(CollectionKind::List(elem)) = &li.col_kind {
                        let at = type_of_expr(&args[0], env, sf, reporter);
                        if !same_type(elem, &at) {
                            reporter.emit(Diagnostic::error(format!(
                                "type mismatch for List.push: expected element {:?}, found {:?}", elem, at
                            )).with_file(sf.path.clone()));
                        }
                    }
                }
            }
        }
        if mname.0 == "put" && args.len() >= 2 {
            if let Expr::Ident(id) = &**obj {
                if let Some(li) = env.get(&id.0) {
                    if let Some(CollectionKind::Map(kt, vt)) = &li.col_kind {
                        let ka = type_of_expr(&args[0], env, sf, reporter);
                        let va = type_of_expr(&args[1], env, sf, reporter);
                        if !same_type(kt, &ka) {
                            reporter.emit(
                                Diagnostic::error(format!(
                                    "type mismatch for Map.put key: expected {:?}, found {:?}",
                                    kt, ka
                                ))
                                .with_file(sf.path.clone()),
                            );
                        }
                        if !same_type(vt, &va) {
                            reporter.emit(
                                Diagnostic::error(format!(
                                    "type mismatch for Map.put value: expected {:?}, found {:?}",
                                    vt, va
                                ))
                                .with_file(sf.path.clone()),
                            );
                        }
                    }
                }
            }
        }

        // Optional safety lint: warn about unsafe .get() on Optional types
        // Users should use .orElse(default), pattern matching, or check isPresent() first
        if mname.0 == "get" || mname.0 == "unwrap" {
            let obj_ty = type_of_expr(obj, env, sf, reporter);
            if is_optional_ty(&obj_ty) {
                reporter.emit(
                    Diagnostic::warning(format!(
                        "calling '{}' on Optional<T> may panic if empty; consider using orElse(default), pattern matching, or check isPresent() first",
                        mname.0
                    ))
                    .with_file(sf.path.clone()),
                );
            }
        }
    } else {
        // Module style: List.push(list, v) or Map.put(m, k, v)
        let mut path: Vec<String> = Vec::new();
        if collect_callee_path(callee, &mut path) {
            if path.len() >= 2 {
                let recv = &path[path.len() - 2];
                let fname2 = &path[path.len() - 1];
                // Enum helpers: Enum.tag(e) / Enum.get(e, i) / Enum.payloadCount(e)
                if recv == "Enum" {
                    if fname2 == "tag" && args.len() >= 1 {
                        let a0 = type_of_expr(&args[0], env, sf, reporter);
                        if !matches!(a0, Ty::Int | Ty::Unknown) {
                            reporter.emit(
                                Diagnostic::error("Enum.tag expects (Int handle)")
                                    .with_file(sf.path.clone()),
                            );
                        }
                    }
                    if fname2 == "get" && args.len() >= 2 {
                        let a0 = type_of_expr(&args[0], env, sf, reporter);
                        let a1 = type_of_expr(&args[1], env, sf, reporter);
                        if !matches!(a0, Ty::Int | Ty::Unknown)
                            || !matches!(a1, Ty::Int | Ty::Unknown)
                        {
                            reporter.emit(
                                Diagnostic::error("Enum.get expects (Int handle, Int index)")
                                    .with_file(sf.path.clone()),
                            );
                        }
                    }
                    if fname2 == "payloadCount" && args.len() >= 1 {
                        let a0 = type_of_expr(&args[0], env, sf, reporter);
                        if !matches!(a0, Ty::Int | Ty::Unknown) {
                            reporter.emit(
                                Diagnostic::error("Enum.payloadCount expects (Int handle)")
                                    .with_file(sf.path.clone()),
                            );
                        }
                    }
                }
                if recv == "List" && fname2 == "push" && args.len() >= 2 {
                    if let Expr::Ident(id) = &args[0] {
                        if let Some(li) = env.get(&id.0) {
                            if let Some(CollectionKind::List(elem)) = &li.col_kind {
                                let at = type_of_expr(&args[1], env, sf, reporter);
                                if !same_type(elem, &at) {
                                    reporter.emit(Diagnostic::error(format!(
                                        "type mismatch for List.push: expected element {:?}, found {:?}", elem, at
                                    )).with_file(sf.path.clone()));
                                }
                            }
                        }
                    }
                }
                if recv == "Map" && fname2 == "put" && args.len() >= 3 {
                    if let Expr::Ident(id) = &args[0] {
                        if let Some(li) = env.get(&id.0) {
                            if let Some(CollectionKind::Map(kt, vt)) = &li.col_kind {
                                let ka = type_of_expr(&args[1], env, sf, reporter);
                                let va = type_of_expr(&args[2], env, sf, reporter);
                                if !same_type(kt, &ka) {
                                    reporter.emit(Diagnostic::error(format!(
                                        "type mismatch for Map.put key: expected {:?}, found {:?}", kt, ka
                                    )).with_file(sf.path.clone()));
                                }
                                if !same_type(vt, &va) {
                                    reporter.emit(Diagnostic::error(format!(
                                        "type mismatch for Map.put value: expected {:?}, found {:?}", vt, va
                                    )).with_file(sf.path.clone()));
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    if handled_enum_constructor {
        return;
    }

    let candidates = lookup_callee_sigs(callee, env);
    if candidates.is_empty() {
        // If hidden only by visibility, report access error
        let (raw, key) = lookup_callee_raw(callee, env);
        if let Some((pkg, module_opt)) = key {
            if !raw.is_empty() {
                let fname = format_callee_for_error(callee, env);
                reporter.emit(
                    Diagnostic::error(format!(
                        "'{}' is not visible here ({}::{}, declared {:?})",
                        fname,
                        pkg,
                        module_opt.clone().unwrap_or_else(|| "<free>".to_string()),
                        raw.first()
                            .map(|s| &s.vis)
                            .unwrap_or(&AS::Visibility::Private)
                    ))
                    .with_file(sf.path.clone()),
                );
                return;
            }
        }
        // Try value call: function-typed variable
        let ct = type_of_expr(callee, env, sf, reporter);
        if let Ty::Function(params, _ret) = ct {
            if params.len() != args.len() {
                reporter.emit(
                    Diagnostic::error(format!(
                        "wrong number of arguments in call: expected {}, found {}",
                        params.len(),
                        args.len()
                    ))
                    .with_file(sf.path.clone()),
                );
                return;
            }
            for (i, (pt, a)) in params.iter().zip(args.iter()).enumerate() {
                let at = type_of_expr(a, env, sf, reporter);
                if !same_type(pt, &at) {
                    reporter.emit(
                        Diagnostic::error(format!(
                            "type mismatch for argument {}: expected {:?}, found {:?}",
                            i + 1,
                            pt,
                            at
                        ))
                        .with_file(sf.path.clone()),
                    );
                }
            }
        }
        return;
    }
    let fname = format_callee_for_error(callee, env);
    let arg_types: Vec<Ty> = args
        .iter()
        .map(|a| type_of_expr(a, env, sf, reporter))
        .collect();
    // Helper to normalize a param type
    let norm_param = |np: &AS::NamePath| {
        let mut t = map_name_to_ty(np);
        if let Ty::Named(ref mut pth) = t {
            qualify_named_with_imports(pth, env);
            // Treat parameters declared as `Fn<...>(...)` as accepting
            // any function-typed value. We don't currently encode full
            // function parameter/return types on FuncSig params, so we
            // normalize `Fn` to Unknown here to avoid spurious type
            // mismatch errors for higher-order helpers like find/any/all.
            if pth.last().map(|s| s.as_str()) == Some("Fn") {
                return Ty::Unknown;
            }
        }
        resolve_alias_ty(t, env)
    };
    let arity_matches: Vec<&AS::FuncSig> = candidates
        .iter()
        .filter(|s| s.params.len() == arg_types.len())
        .collect();
    // Score exact matches by counting exact param matches (Unknown tolerated but not counted as exact)
    let mut best: Option<(&AS::FuncSig, usize)> = None;
    let mut ties = 0usize;
    for s in &arity_matches {
        // Extract type parameter names from generics
        let type_params: Vec<String> = s.generics.iter().map(|g| g.name.0.clone()).collect();

        let mut exact = 0usize;
        let mut ok = true;
        let mut subst: HashMap<String, Ty> = HashMap::new();

        for (i, p) in s.params.iter().enumerate() {
            let pt = norm_param(&p.ty);
            let at = &arg_types[i];
            if matches!(at, Ty::Unknown) {
                continue; // wildcard, does not increase exact score
            }

            // Use type parameter inference if this is a generic function
            if !type_params.is_empty() {
                if match_types_with_inference(&pt, at, &type_params, &mut subst) {
                    // Count only when both sides are not Unknown and not a type parameter
                    if !matches!(pt, Ty::Unknown) && !is_type_param_ty(&pt, &type_params) {
                        exact += 1;
                    }
                } else {
                    ok = false;
                    break;
                }
            } else if same_type(&pt, at) {
                // Count only when both sides are not Unknown
                if !matches!(pt, Ty::Unknown) {
                    exact += 1;
                }
            } else {
                ok = false;
                break;
            }
        }
        if ok {
            match &mut best {
                None => {
                    best = Some((s, exact));
                    ties = 1;
                }
                Some((_, cur)) => {
                    if exact > *cur {
                        best = Some((s, exact));
                        ties = 1;
                    } else if exact == *cur {
                        ties += 1;
                    }
                }
            }
        }
    }
    if let Some((_, _)) = best {
        if ties > 1 {
            reporter.emit(
                Diagnostic::error(format!("ambiguous call to '{}' among overloads", fname))
                    .with_file(sf.path.clone()),
            );
            return;
        }
        // Selected; nothing more to check (types already compatible by exactness). Additional collection checks ran above.
        return;
    }
    // No overload matched; decide on message
    if arity_matches.is_empty() {
        // Show an example expected arity if any overloads exist
        let mut arities: Vec<usize> = candidates.iter().map(|s| s.params.len()).collect();
        arities.sort();
        arities.dedup();
        let expected = arities
            .iter()
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
            .join("/ ");
        reporter.emit(
            Diagnostic::error(format!(
                "wrong number of arguments in call to '{}': found {}; available arities: {}",
                fname,
                args.len(),
                expected
            ))
            .with_file(sf.path.clone()),
        );
        return;
    } else {
        // Arity matches exist but param types mismatched. Report first mismatch detail.
        // Compare against the first arity-matched signature for a concrete message.
        let s = arity_matches[0];
        // Extract type parameter names from generics
        let type_params: Vec<String> = s.generics.iter().map(|g| g.name.0.clone()).collect();
        let mut subst: HashMap<String, Ty> = HashMap::new();

        for (idx, p) in s.params.iter().enumerate() {
            let pt = norm_param(&p.ty);
            let at = &arg_types[idx];

            // Use type parameter inference for generic functions
            let types_match = if !type_params.is_empty() {
                match_types_with_inference(&pt, at, &type_params, &mut subst)
            } else {
                same_type(&pt, at)
            };

            if !types_match {
                reporter.emit(
                    Diagnostic::error(format!(
                        "type mismatch for argument {} in call to '{}': expected {:?}, found {:?}",
                        idx + 1,
                        fname,
                        pt,
                        at
                    ))
                    .with_file(sf.path.clone()),
                );
                break;
            }
        }
        return;
    }
}

fn last_segment_of_named(t: &Ty) -> Option<&str> {
    match t {
        Ty::Named(path) => path.last().map(|s| s.as_str()),
        _ => None,
    }
}

fn is_shared_like(t: &Ty) -> bool {
    matches!(
        last_segment_of_named(t),
        Some("Shared") | Some("Atomic") | Some("Watch") | Some("Notify")
    )
}

fn is_owned_like(t: &Ty) -> bool {
    matches!(last_segment_of_named(t), Some("Owned"))
}

fn is_capability_ty(t: &Ty) -> bool {
    matches!(last_segment_of_named(t), Some("Cap"))
}

fn has_capability_arg(args: &[Expr], env: &Env) -> bool {
    for a in args {
        // Only simple identifiers are tracked in env; more complex expressions are Unknown
        if let Expr::Ident(id) = a
            && let Some(li) = env.get(&id.0)
            && is_capability_ty(&li.ty)
        {
            return true;
        }
    }
    false
}

/// Check if a function call requires unsafe context (extern or unsafe function)
fn unsafe_check_call(
    callee: &Expr,
    env: &Env,
    sf: &SourceFile,
    reporter: &mut Reporter,
    current_pkg: &str,
) {
    // If already in unsafe context, no need to check
    if env.is_unsafe_context() {
        return;
    }

    let mut path: Vec<String> = Vec::new();
    if !collect_callee_path(callee, &mut path) {
        return;
    }

    // Resolve the full function path to check against extern/unsafe indices
    let func_name = path.last().cloned().unwrap_or_default();

    // Check for extern function: extern functions are top-level in a package
    // Path could be: ["extern_func"] (same package) or ["pkg", "extern_func"]
    if path.len() == 1 {
        // Same package extern function
        let key = (current_pkg.to_string(), func_name.clone());
        if env.extern_funcs.contains_key(&key) {
            reporter.emit(
                Diagnostic::error(format!(
                    "call to extern function '{}' requires unsafe context; wrap the call in an unsafe block or mark the enclosing function as unsafe",
                    func_name
                ))
                .with_file(sf.path.clone()),
            );
            return;
        }
    } else if path.len() >= 2 {
        // Qualified extern function: pkg.extern_func
        let pkg = path[..path.len() - 1].join(".");
        let key = (pkg.clone(), func_name.clone());
        if env.extern_funcs.contains_key(&key) {
            reporter.emit(
                Diagnostic::error(format!(
                    "call to extern function '{}.{}' requires unsafe context; wrap the call in an unsafe block or mark the enclosing function as unsafe",
                    pkg, func_name
                ))
                .with_file(sf.path.clone()),
            );
            return;
        }
    }

    // Check for unsafe function calls
    // Could be: ["unsafe_func"] (same module), ["Module", "unsafe_func"], or ["pkg", "Module", "unsafe_func"]
    if path.len() == 1 {
        // Same module function - check with current module
        let key = (
            current_pkg.to_string(),
            env.current_module.clone(),
            func_name.clone(),
        );
        if env.unsafe_funcs.contains(&key) {
            reporter.emit(
                Diagnostic::error(format!(
                    "call to unsafe function '{}' requires unsafe context; wrap the call in an unsafe block or mark the enclosing function as unsafe",
                    func_name
                ))
                .with_file(sf.path.clone()),
            );
            return;
        }
        // Also check as a free function (None for module)
        let key_free = (current_pkg.to_string(), None, func_name.clone());
        if env.unsafe_funcs.contains(&key_free) {
            reporter.emit(
                Diagnostic::error(format!(
                    "call to unsafe function '{}' requires unsafe context; wrap the call in an unsafe block or mark the enclosing function as unsafe",
                    func_name
                ))
                .with_file(sf.path.clone()),
            );
            return;
        }
    } else if path.len() == 2 {
        // Module.func in same package or pkg.func
        let module_or_pkg = &path[0];

        // Try as Module.func (same package)
        // First check if it's a module name (via imported modules or star imports)
        let resolved_pkg = env
            .imported_modules
            .get(module_or_pkg)
            .cloned()
            .unwrap_or_else(|| current_pkg.to_string());

        let key = (
            resolved_pkg.clone(),
            Some(module_or_pkg.clone()),
            func_name.clone(),
        );
        if env.unsafe_funcs.contains(&key) {
            reporter.emit(
                Diagnostic::error(format!(
                    "call to unsafe function '{}.{}' requires unsafe context; wrap the call in an unsafe block or mark the enclosing function as unsafe",
                    module_or_pkg, func_name
                ))
                .with_file(sf.path.clone()),
            );
            return;
        }

        // Also try as same-package module
        let key_same_pkg = (
            current_pkg.to_string(),
            Some(module_or_pkg.clone()),
            func_name.clone(),
        );
        if env.unsafe_funcs.contains(&key_same_pkg) {
            reporter.emit(
                Diagnostic::error(format!(
                    "call to unsafe function '{}.{}' requires unsafe context; wrap the call in an unsafe block or mark the enclosing function as unsafe",
                    module_or_pkg, func_name
                ))
                .with_file(sf.path.clone()),
            );
            return;
        }
    } else if path.len() >= 3 {
        // pkg.Module.func
        let pkg = path[..path.len() - 2].join(".");
        let module_name = &path[path.len() - 2];
        let key = (pkg.clone(), Some(module_name.clone()), func_name.clone());
        if env.unsafe_funcs.contains(&key) {
            reporter.emit(
                Diagnostic::error(format!(
                    "call to unsafe function '{}.{}.{}' requires unsafe context; wrap the call in an unsafe block or mark the enclosing function as unsafe",
                    pkg, module_name, func_name
                ))
                .with_file(sf.path.clone()),
            );
        }
    }
}

/// Check if arguments to an extern function call violate FFI move restrictions.
/// Arth-owned types (String, structs, enums, collections) cannot be moved to C code.
fn ffi_move_check_call(
    callee: &Expr,
    args: &[Expr],
    env: &Env,
    sf: &SourceFile,
    reporter: &mut Reporter,
    current_pkg: &str,
) {
    let mut path: Vec<String> = Vec::new();
    if !collect_callee_path(callee, &mut path) {
        return;
    }

    let func_name = path.last().cloned().unwrap_or_default();

    // Find the extern function signature
    let extern_key = if path.len() == 1 {
        (current_pkg.to_string(), func_name.clone())
    } else {
        let pkg = path[..path.len() - 1].join(".");
        (pkg, func_name.clone())
    };

    // Only check if this is actually an extern function
    let Some(_extern_info) = env.extern_funcs.get(&extern_key) else {
        return;
    };

    // Check each argument's type for FFI safety
    for (i, arg) in args.iter().enumerate() {
        let arg_ty = type_of_expr(arg, env, sf, reporter);

        if !is_ffi_safe_ty(&arg_ty) {
            let reason = ffi_unsafe_reason(&arg_ty);
            let display_name = if path.len() == 1 {
                func_name.clone()
            } else {
                format!("{}.{}", path[..path.len() - 1].join("."), func_name)
            };
            reporter.emit(
                Diagnostic::error(format!(
                    "cannot pass Arth-owned value of type '{}' to extern function '{}' (argument {}): {}",
                    ty_to_string(&arg_ty),
                    display_name,
                    i + 1,
                    reason
                ))
                .with_file(sf.path.clone()),
            );
        }
    }
}

/// Check if arguments to an extern function call in async context are Sendable.
/// In async functions, the scheduler may move execution between threads, so values
/// passed to FFI must be Sendable to ensure thread safety.
fn ffi_sendable_check_call(
    callee: &Expr,
    args: &[Expr],
    env: &Env,
    sf: &SourceFile,
    reporter: &mut Reporter,
    current_pkg: &str,
) {
    // Only check in async context
    if !env.in_async {
        return;
    }

    let mut path: Vec<String> = Vec::new();
    if !collect_callee_path(callee, &mut path) {
        return;
    }

    let func_name = path.last().cloned().unwrap_or_default();

    // Find the extern function signature
    let extern_key = if path.len() == 1 {
        (current_pkg.to_string(), func_name.clone())
    } else {
        let pkg = path[..path.len() - 1].join(".");
        (pkg, func_name.clone())
    };

    // Only check if this is actually an extern function
    if env.extern_funcs.get(&extern_key).is_none() {
        return;
    }

    let display_name = if path.len() == 1 {
        func_name.clone()
    } else {
        format!("{}.{}", path[..path.len() - 1].join("."), func_name)
    };

    // Check each argument's type for Sendable in async context
    for (i, arg) in args.iter().enumerate() {
        let arg_ty = type_of_expr(arg, env, sf, reporter);

        // Skip primitives - always Sendable
        if matches!(
            arg_ty,
            Ty::Int | Ty::Float | Ty::Bool | Ty::Char | Ty::String | Ty::Bytes | Ty::Void
        ) {
            continue;
        }

        if !is_sendable(env, &arg_ty) {
            reporter.emit(
                Diagnostic::error(format!(
                    "argument {} of type '{}' to extern function '{}' is not Sendable; \
                     in async context, all values passed to FFI must be Sendable \
                     because the scheduler may move execution between threads",
                    i + 1,
                    ty_to_string(&arg_ty),
                    display_name
                ))
                .with_file(sf.path.clone()),
            );
        }
    }
}

fn effect_check_call(
    callee: &Expr,
    args: &[Expr],
    env: &mut Env,
    sf: &SourceFile,
    reporter: &mut Reporter,
    current_pkg: &str,
    rp: &crate::compiler::resolve::ResolvedProgram,
) {
    // Unsafe context check for extern and unsafe function calls
    unsafe_check_call(callee, env, sf, reporter, current_pkg);

    // FFI move restriction check for extern function calls
    ffi_move_check_call(callee, args, env, sf, reporter, current_pkg);

    // Sendable check for extern function calls in async context
    ffi_sendable_check_call(callee, args, env, sf, reporter, current_pkg);

    // Only check simple names for now.
    let Some(name) = name_of_callee(callee) else {
        return;
    };
    // Heuristic: treat std collection helpers (List/Map) as self‑guarded; no Cap required.
    // Allows calls like List.push(xs, v), Map.put(m, k, v) and method sugar xs.push(...), m.put(...)
    {
        let mut path: Vec<String> = Vec::new();
        if collect_callee_path(callee, &mut path) {
            if path.len() >= 2 {
                let recv_name = &path[path.len() - 2];
                if recv_name == "List" || recv_name == "Map" {
                    return;
                }
            }
        }
        // Member sugar: if callee is 'obj.push' or 'obj.put' and obj type is List/Map, skip capability requirement
        if let Expr::Member(obj, m) = callee {
            if matches!(m.0.as_str(), "push" | "put") {
                let ty = type_of_expr(obj, env, sf, reporter);
                if matches!(ty, Ty::Named(_)) {
                    if let Some(last) = last_segment_of_named(&ty) {
                        if last == "List" || last == "Map" {
                            return;
                        }
                    }
                }
            }
        }
    }
    // Await boundary is handled when visiting Expr::Await
    // Borrow helpers: track exclusive and provider-tied borrows.
    if name == "borrowMut" {
        if let Some(Expr::Ident(id)) = args.first() {
            // Check for conflicts with existing borrows using Env's direct tracking
            if let Some(err_msg) = env.check_borrow_conflict(&id.0, BorrowAction::ExclusiveBorrow) {
                let suggestion = if env.has_shared_borrows(&id.0) {
                    format!(
                        "ensure all field accesses on '{}' are complete before calling borrowMut",
                        id.0
                    )
                } else {
                    format!("call release({}) before re-borrowing", id.0)
                };
                reporter.emit(
                    Diagnostic::error(err_msg)
                        .with_file(sf.path.clone())
                        .with_suggestion(suggestion),
                );
                return;
            }

            let already = is_excl_borrowed(env, &id.0);
            if let Some(top) = env.excl_borrows.last_mut() {
                if already {
                    reporter.emit(
                        Diagnostic::error(format!(
                            "cannot take an exclusive borrow of '{}' while another exclusive borrow is active",
                            id.0
                        ))
                        .with_file(sf.path.clone())
                        .with_suggestion(format!("call release({}) before re-borrowing", id.0)),
                    );
                } else {
                    top.insert(id.0.clone());
                }
            }
            // Also track in lifetime environment for region-based analysis
            if let Err(err) = env.lifetime_env.borrow_exclusive(&id.0, None, None) {
                reporter.emit(Diagnostic::error(err.to_message()).with_file(sf.path.clone()));
            }
        }
        return;
    }
    if name == "borrowFromProvider" {
        if let Some(arg0) = args.first() {
            let ty0 = type_of_expr(arg0, env, sf, reporter);
            if let Ty::Named(path) = ty0 {
                let provider_name = if path.len() == 1 {
                    path[0].clone()
                } else {
                    // Fully qualified: pkg.ProviderName -> use last segment
                    path.last().unwrap().clone()
                };

                let (pkg, _nm) = if path.len() == 1 {
                    (
                        env.current_pkg
                            .clone()
                            .unwrap_or_else(|| current_pkg.to_string()),
                        path[0].clone(),
                    )
                } else {
                    (
                        path[..path.len() - 1].join("."),
                        path.last().unwrap().clone(),
                    )
                };

                // Verify this is actually a provider
                if let Some(crate::compiler::resolve::ResolvedKind::Provider) =
                    crate::compiler::resolve::lookup_symbol_kind(rp, &pkg, &path)
                {
                    // Track in prov_borrows set (for function-exit validation)
                    if let Some(top) = env.prov_borrows.last_mut() {
                        top.insert(provider_name.clone());
                    }

                    // Create the provider borrow in the lifetime environment
                    // This creates a BorrowOrigin::Provider which is allowed to "escape" function scope
                    // The holder will be tracked when the result is assigned to a variable
                    let _region = env.lifetime_env.borrow_from_provider(
                        &provider_name,
                        None,                         // holder tracked at assignment site
                        lifetime::BorrowMode::Shared, // default to shared; exclusive would use borrowMut pattern
                        None, // span could be extracted from arg0 if available
                    );
                }
            }
        }
        return;
    }
    if name == "release" {
        if let Some(Expr::Ident(id)) = args.first() {
            if let Some(top) = env.excl_borrows.last_mut() {
                top.remove(&id.0);
            }
            if let Some(top) = env.prov_borrows.last_mut() {
                top.remove(&id.0);
            }
            // Also release in lifetime environment
            env.lifetime_env.release_borrow(&id.0);
        }
        return;
    }
    // Common mutator names (provider-guarded when used with Shared/Atomic handles)
    const MUTATORS: &[&str] = &[
        "put", "remove", "insert", "set", "push", "pop", "clear", "update", "swap", "append",
        "prepend", "add", "delete", "offer", "poll", "write", "inc", "dec", "publish",
    ];
    if !MUTATORS.contains(&name.as_str()) {
        return;
    }

    // Must have a receiver-like first arg
    if args.is_empty() {
        return;
    }
    let recv_ty = type_of_expr(&args[0], env, sf, reporter);
    if matches!(recv_ty, Ty::Unknown) {
        return;
    }
    // Special handling for observable/atomic wrappers
    if let Some(recv_last) = last_segment_of_named(&recv_ty) {
        match recv_last {
            // Atomic operations are self-guarded
            "Atomic" => return,
            // Watch mutations require capability tokens
            "Watch" => {
                if matches!(name.as_str(), "set" | "update") {
                    if has_capability_arg(args, env) {
                        return;
                    }
                    reporter.emit(
                        Diagnostic::error(
                            "Watch mutation requires capability token (Cap<Write<...>>)",
                        )
                        .with_file(sf.path.clone()),
                    );
                    return;
                } else {
                    return; // non-mutating ops on Watch
                }
            }
            // Notify publishing requires capability tokens
            "Notify" => {
                if matches!(name.as_str(), "publish" | "emit" | "post") {
                    if has_capability_arg(args, env) {
                        return;
                    }
                    reporter.emit(
                        Diagnostic::error(
                            "Notify publish requires capability token (Cap<Emit<...>>)",
                        )
                        .with_file(sf.path.clone()),
                    );
                    return;
                } else {
                    return; // subscribe/read-like ops
                }
            }
            // Generic Shared<T>: assume provider guards mutation via actors/capabilities
            "Shared" => return,
            _ => {}
        }
    }
    if is_owned_like(&recv_ty) {
        // Owned<T>: exclusive; allowed.
        return;
    }
    // Mutators on primitives/strings/bytes are ignored; named types require capabilities unless wrapped.
    if matches!(
        recv_ty,
        Ty::Int | Ty::Float | Ty::Bool | Ty::Char | Ty::String | Ty::Bytes
    ) {
        return;
    }
    // Non-wrapper, non-primitive named types: require a capability token to authorize mutation
    if has_capability_arg(args, env) {
        return;
    }
    reporter.emit(Diagnostic::error(format!(
        "mutating operation '{}' requires a capability token (Cap<...>) or a provider-guarded handle (Owned<T>/Shared<T>/Atomic<T>)",
        name
    )).with_file(sf.path.clone()));
}

fn join_path(path: &[String]) -> String {
    path.join(".")
}

/// Returns (Sendable, Shareable, UnwindSafe) for a named type.
fn conc_for_named(env: &Env, path: &[String]) -> Option<(bool, bool, bool)> {
    if path.is_empty() {
        return None;
    }
    // Wrappers - return (Sendable, Shareable, UnwindSafe)
    let last = path.last().unwrap();
    // Shared: not sendable, shareable, NOT unwind-safe (interior mutability)
    if last == "Shared" {
        return Some((false, true, false));
    }
    // Atomic: sendable, shareable, unwind-safe (lock-free atomic ops)
    if last == "Atomic" {
        return Some((true, true, true));
    }
    // Watch: not sendable, shareable, NOT unwind-safe (interior mutability)
    if last == "Watch" {
        return Some((false, true, false));
    }
    // Notify: sendable, shareable, unwind-safe (no mutable state)
    if last == "Notify" {
        return Some((true, true, true));
    }
    // Owned: sendable, not shareable, unwind-safe
    if last == "Owned" {
        return Some((true, false, true));
    }
    // Compute (pkg, name)
    let (pkg, name) = if path.len() == 1 {
        (env.current_pkg.clone()?, path[0].clone())
    } else {
        (
            path[..path.len() - 1].join("."),
            path.last().unwrap().clone(),
        )
    };
    env.conc.get(&(pkg, name)).cloned()
}

fn is_enum_named(env: &Env, path: &[String]) -> bool {
    if path.is_empty() {
        return false;
    }
    let (pkg, name) = if path.len() == 1 {
        (
            env.current_pkg.clone().unwrap_or_else(|| String::new()),
            path[0].clone(),
        )
    } else {
        (
            path[..path.len() - 1].join("."),
            path.last().unwrap().clone(),
        )
    };
    env.enum_variants
        .keys()
        .any(|(p, n, _)| p == &pkg && n == &name)
}

/// Get all variant names for an enum type.
/// Returns (pkg, enum_name, Vec<variant_name>).
fn get_enum_variants(env: &Env, path: &[String]) -> Option<(String, String, Vec<String>)> {
    if path.is_empty() {
        return None;
    }
    let (pkg, name) = if path.len() == 1 {
        (
            env.current_pkg.clone().unwrap_or_else(|| String::new()),
            path[0].clone(),
        )
    } else {
        (
            path[..path.len() - 1].join("."),
            path.last().unwrap().clone(),
        )
    };
    let variants: Vec<String> = env
        .enum_variants
        .keys()
        .filter(|(p, n, _)| p == &pkg && n == &name)
        .map(|(_, _, v)| v.clone())
        .collect();
    if variants.is_empty() {
        None
    } else {
        Some((pkg, name, variants))
    }
}

/// Look up an enum variant by enum name and variant name.
/// Searches: current package, star-imported packages, and explicitly imported modules.
/// Returns (enum_pkg, enum_name) if found, None otherwise.
fn lookup_enum_variant(env: &Env, enum_name: &str, variant_name: &str) -> Option<(String, String)> {
    let current_pkg = env.current_pkg.clone().unwrap_or_default();

    // 1. Check current package
    let key = (
        current_pkg.clone(),
        enum_name.to_string(),
        variant_name.to_string(),
    );
    if env.enum_variants.contains_key(&key) {
        return Some((current_pkg.clone(), enum_name.to_string()));
    }

    // 2. Check star-imported packages
    for pkg in env.star_import_pkgs.iter() {
        let key = (pkg.clone(), enum_name.to_string(), variant_name.to_string());
        if env.enum_variants.contains_key(&key) {
            return Some((pkg.clone(), enum_name.to_string()));
        }
    }

    // 3. Check if enum_name is an explicitly imported module/type
    if let Some(pkg) = env.imported_modules.get(enum_name) {
        let key = (pkg.clone(), enum_name.to_string(), variant_name.to_string());
        if env.enum_variants.contains_key(&key) {
            return Some((pkg.clone(), enum_name.to_string()));
        }
    }

    None
}

/// Get payload types for an enum variant.
/// Returns Vec of payload NamePaths (empty for unit variants).
fn get_enum_variant_payloads(
    env: &Env,
    enum_name: &str,
    variant_name: &str,
) -> Option<Vec<AS::NamePath>> {
    let current_pkg = env.current_pkg.clone().unwrap_or_default();

    // 1. Check current package
    let key = (
        current_pkg.clone(),
        enum_name.to_string(),
        variant_name.to_string(),
    );
    if let Some(payloads) = env.enum_variants.get(&key) {
        return Some(payloads.clone());
    }

    // 2. Check star-imported packages
    for pkg in env.star_import_pkgs.iter() {
        let key = (pkg.clone(), enum_name.to_string(), variant_name.to_string());
        if let Some(payloads) = env.enum_variants.get(&key) {
            return Some(payloads.clone());
        }
    }

    // 3. Check explicitly imported module/type
    if let Some(pkg) = env.imported_modules.get(enum_name) {
        let key = (pkg.clone(), enum_name.to_string(), variant_name.to_string());
        if let Some(payloads) = env.enum_variants.get(&key) {
            return Some(payloads.clone());
        }
    }

    None
}

/// Type check a pattern and return bindings to introduce into scope.
/// Returns Vec<(binding_name, binding_type)> for variables bound by the pattern.
fn typecheck_pattern(
    pat: &AS::Pattern,
    scrutinee_ty: &Ty,
    env: &Env,
    sf: &SourceFile,
    reporter: &mut Reporter,
) -> Vec<(String, Ty)> {
    use AS::Pattern;
    match pat {
        Pattern::Wildcard => {
            // Wildcard matches anything, binds nothing
            vec![]
        }
        Pattern::Binding(ident) => {
            // Binding captures the entire scrutinee value
            vec![(ident.0.clone(), scrutinee_ty.clone())]
        }
        Pattern::Literal(expr) => {
            // Check for null in pattern - Arth has no null
            if let AS::Expr::Ident(id) = expr.as_ref() {
                if id.0 == "null" {
                    reporter.emit(
                        Diagnostic::error(
                            "null is not allowed in patterns; use Optional<T> with 'case None' or 'case Some(value)' instead"
                        )
                        .with_file(sf.path.clone()),
                    );
                    return vec![];
                }
            }

            // Literal pattern: check type matches scrutinee
            let lit_ty = type_of_expr(expr, env, sf, reporter);
            if !same_type(scrutinee_ty, &lit_ty) {
                reporter.emit(
                    Diagnostic::error(format!(
                        "pattern type mismatch: expected {:?}, found {:?}",
                        scrutinee_ty, lit_ty
                    ))
                    .with_file(sf.path.clone()),
                );
            }
            vec![]
        }
        Pattern::Variant {
            enum_ty,
            variant,
            payloads,
            ..
        } => {
            // Enum variant pattern: EnumType.Variant(p1, p2, ...)
            let enum_path: Vec<String> = enum_ty.path.iter().map(|i| i.0.clone()).collect();

            // Resolve package for the enum, accepting unqualified names in pattern cases.
            let current_pkg = env.current_pkg.clone().unwrap_or_default();
            let (pkg, enum_name, resolved_enum_path) = if enum_path.len() == 1 {
                let enum_name = enum_path[0].clone();
                if let Some((resolved_pkg, resolved_enum)) =
                    lookup_enum_variant(env, &enum_name, &variant.0)
                {
                    let mut p: Vec<String> = if resolved_pkg.is_empty() {
                        vec![]
                    } else {
                        resolved_pkg.split('.').map(|s| s.to_string()).collect()
                    };
                    p.push(resolved_enum.clone());
                    (resolved_pkg, resolved_enum, p)
                } else {
                    (
                        current_pkg.clone(),
                        enum_name.clone(),
                        vec![enum_name.clone()],
                    )
                }
            } else {
                (
                    enum_path[..enum_path.len() - 1].join("."),
                    enum_path.last().unwrap().clone(),
                    enum_path.clone(),
                )
            };

            // Verify scrutinee type matches the enum type in the pattern
            let expected_scrutinee = Ty::Named(resolved_enum_path);
            if !same_type(scrutinee_ty, &expected_scrutinee) {
                reporter.emit(
                    Diagnostic::error(format!(
                        "pattern enum type mismatch: switch expression is {:?}, but pattern matches {:?}",
                        scrutinee_ty, expected_scrutinee
                    ))
                    .with_file(sf.path.clone()),
                );
            }

            // Look up the variant
            let key = (pkg.clone(), enum_name.clone(), variant.0.clone());
            if let Some(payload_types) = env.enum_variants.get(&key) {
                // Check payload count matches
                if payloads.len() != payload_types.len() {
                    reporter.emit(
                        Diagnostic::error(format!(
                            "variant {}.{} expects {} payload(s), found {}",
                            enum_name,
                            variant.0,
                            payload_types.len(),
                            payloads.len()
                        ))
                        .with_file(sf.path.clone()),
                    );
                }

                // Type check each payload pattern and collect bindings
                let mut bindings = vec![];
                for (i, (sub_pat, payload_ty_np)) in
                    payloads.iter().zip(payload_types.iter()).enumerate()
                {
                    // Convert NamePath to Ty
                    let payload_ty = namepath_to_ty(payload_ty_np);
                    let sub_bindings = typecheck_pattern(sub_pat, &payload_ty, env, sf, reporter);
                    bindings.extend(sub_bindings);

                    // Check for wildcard in wrong position (optional validation)
                    if i >= payload_types.len() {
                        reporter.emit(
                            Diagnostic::error(format!(
                                "extra payload pattern at index {} for variant {}.{}",
                                i, enum_name, variant.0
                            ))
                            .with_file(sf.path.clone()),
                        );
                    }
                }
                bindings
            } else {
                reporter.emit(
                    Diagnostic::error(format!("unknown enum variant: {}.{}", enum_name, variant.0))
                        .with_file(sf.path.clone()),
                );
                vec![]
            }
        }
    }
}

/// Convert an AST NamePath to a Ty
fn namepath_to_ty(np: &AS::NamePath) -> Ty {
    let parts: Vec<String> = np.path.iter().map(|i| i.0.clone()).collect();
    if parts.len() == 1 {
        match parts[0].as_str() {
            "int" | "Int" => Ty::Int,
            "float" | "Float" => Ty::Float,
            "bool" | "Bool" => Ty::Bool,
            "char" | "Char" => Ty::Char,
            "String" => Ty::String,
            "void" | "Void" => Ty::Void,
            _ => Ty::Named(parts),
        }
    } else {
        Ty::Named(parts)
    }
}

fn is_sendable(env: &Env, ty: &Ty) -> bool {
    match ty {
        Ty::Int | Ty::Float | Ty::Bool | Ty::Char | Ty::String | Ty::Bytes | Ty::Void => true,
        Ty::Named(path) => conc_for_named(env, path)
            .map(|(s, _, _)| s)
            .unwrap_or(false),
        // Generic types: check the base type and all type arguments
        Ty::Generic { path, args } => {
            // First check the base type (wrapper types have special handling)
            let base_sendable = conc_for_named(env, path)
                .map(|(s, _, _)| s)
                .unwrap_or(false);
            if !base_sendable {
                return false;
            }
            // All type arguments must also be Sendable
            args.iter().all(|arg| is_sendable(env, arg))
        }
        // Tuple types are Sendable if all elements are Sendable
        Ty::Tuple(elems) => elems.iter().all(|e| is_sendable(env, e)),
        // Function types are not Sendable (may capture non-Sendable state)
        Ty::Function(_, _) => false,
        // Reference types: only shared references to Shareable types are Sendable
        Ty::Ref { inner, mode, .. } => {
            match mode {
                BorrowMode::Shared => is_shareable(env, inner),
                // Exclusive borrows are never Sendable (can only exist in one place)
                BorrowMode::Exclusive => false,
            }
        }
        // Never type is trivially Sendable (code never reaches there)
        Ty::Never => true,
        Ty::Unknown => false,
    }
}

fn is_shareable(env: &Env, ty: &Ty) -> bool {
    match ty {
        Ty::Int | Ty::Float | Ty::Bool | Ty::Char | Ty::String | Ty::Bytes | Ty::Void => true,
        Ty::Named(path) => conc_for_named(env, path)
            .map(|(_, sh, _)| sh)
            .unwrap_or(false),
        // Generic types: check the base type and all type arguments
        Ty::Generic { path, args } => {
            // First check the base type (wrapper types have special handling)
            let base_shareable = conc_for_named(env, path)
                .map(|(_, sh, _)| sh)
                .unwrap_or(false);
            if !base_shareable {
                return false;
            }
            // All type arguments must also be Shareable
            args.iter().all(|arg| is_shareable(env, arg))
        }
        // Tuple types are Shareable if all elements are Shareable
        Ty::Tuple(elems) => elems.iter().all(|e| is_shareable(env, e)),
        // Function types are not Shareable (may capture non-Shareable state)
        Ty::Function(_, _) => false,
        // Reference types: only shared references to Shareable types are Shareable
        Ty::Ref { inner, mode, .. } => {
            match mode {
                BorrowMode::Shared => is_shareable(env, inner),
                // Exclusive borrows are never Shareable (mutable aliasing is forbidden)
                BorrowMode::Exclusive => false,
            }
        }
        // Never type is trivially Shareable (code never reaches there)
        Ty::Never => true,
        Ty::Unknown => false,
    }
}

/// Checks if a type is UnwindSafe.
/// UnwindSafe types can be safely held across panic boundaries without violating invariants.
/// Types with interior mutability (Shared, Watch) are NOT unwind-safe because their
/// internal state might be in an inconsistent state after a panic.
fn is_unwind_safe(env: &Env, ty: &Ty) -> bool {
    match ty {
        // Primitives are always UnwindSafe
        Ty::Int | Ty::Float | Ty::Bool | Ty::Char | Ty::String | Ty::Bytes | Ty::Void => true,
        Ty::Named(path) => conc_for_named(env, path).map(|(_, _, u)| u).unwrap_or(true),
        // Generic types: check the base type and all type arguments
        Ty::Generic { path, args } => {
            // First check the base type (wrapper types have special handling)
            let base_unwind_safe = conc_for_named(env, path).map(|(_, _, u)| u).unwrap_or(true);
            if !base_unwind_safe {
                return false;
            }
            // All type arguments must also be UnwindSafe
            args.iter().all(|arg| is_unwind_safe(env, arg))
        }
        // Tuple types are UnwindSafe if all elements are UnwindSafe
        Ty::Tuple(elems) => elems.iter().all(|e| is_unwind_safe(env, e)),
        // Function types are UnwindSafe (they don't hold mutable state themselves)
        Ty::Function(_, _) => true,
        // Reference types: shared references are always UnwindSafe,
        // exclusive references are NOT UnwindSafe (mutable state across unwind)
        Ty::Ref { mode, .. } => {
            match mode {
                BorrowMode::Shared => true,
                // Exclusive borrows are NOT unwind-safe (might observe partially-mutated state)
                BorrowMode::Exclusive => false,
            }
        }
        // Never type is trivially UnwindSafe (code never reaches there)
        Ty::Never => true,
        Ty::Unknown => true, // Default to true for unknown types
    }
}

fn concurrency_check_call(
    callee: &Expr,
    args: &[Expr],
    env: &mut Env,
    sf: &SourceFile,
    reporter: &mut Reporter,
) {
    let Some(name) = name_of_callee(callee) else {
        return;
    };
    let qualified = qualified_callee_name(callee);

    // Spawn-like operations: data is transferred to a new task
    const SPAWN_NAMES: &[&str] = &[
        "spawn",
        "spawnBlocking",
        "spawnWithArg",
        "spawnAwait",
        "startTask",
        "spawnTask",
    ];

    // Send-like operations: data is sent across task boundaries (channels, actors)
    const SEND_NAMES: &[&str] = &[
        "send",
        "sendBlocking",
        "sendWithTask",
        "sendAndWake",
        "trySend",
        "offer",
        "post",
        "emit",
    ];

    let is_spawn = SPAWN_NAMES.contains(&name.as_str());
    let is_send = SEND_NAMES.contains(&name.as_str());
    if !is_spawn && !is_send {
        return;
    }

    // Determine which arguments need Sendable checking based on the operation
    // For most operations, all non-primitive args must be Sendable
    // For Actor.send(handle, value) and MpmcChan.send(handle, value), only value needs checking
    // but handles are Int anyway, so checking all is fine
    for (idx, a) in args.iter().enumerate() {
        let aty = type_of_expr(a, env, sf, reporter);
        // Only consider non-primitive types; primitives are always Sendable
        if matches!(
            aty,
            Ty::Int | Ty::Float | Ty::Bool | Ty::Char | Ty::String | Ty::Bytes
        ) {
            continue;
        }
        if is_shared_like(&aty) {
            // Shared/Atomic handles are shareable by construction
            if !is_shareable(env, &aty) {
                reporter.emit(
                    Diagnostic::error("shared handle is not shareable across tasks")
                        .with_file(sf.path.clone()),
                );
            }
            continue;
        }
        // For spawn/send, require Sendable to move into another task
        if !is_sendable(env, &aty) {
            let tname = match &aty {
                Ty::Named(p) => join_path(p),
                _ => format!("{:?}", aty),
            };
            let op_name = qualified.as_deref().unwrap_or(&name);
            let op_kind = if is_spawn {
                "spawn"
            } else {
                "channel/actor send"
            };
            reporter.emit(
                Diagnostic::error(format!(
                    "argument {} of type '{}' is not Sendable in call to {}; \
                     {} operations require all transferred data to be Sendable",
                    idx + 1,
                    tname,
                    op_name,
                    op_kind
                ))
                .with_file(sf.path.clone()),
            );
        }
    }
}

fn typecheck_block(
    b: &Block,
    env: &mut Env,
    sf: &SourceFile,
    reporter: &mut Reporter,
    current_pkg: &str,
    rp: &crate::compiler::resolve::ResolvedProgram,
) {
    for st in &b.stmts {
        let _ = env.lifetime_env.nll_advance();
        typecheck_stmt(st, env, sf, reporter, current_pkg, rp);
    }
}

fn typecheck_stmt(
    s: &Stmt,
    env: &mut Env,
    sf: &SourceFile,
    reporter: &mut Reporter,
    current_pkg: &str,
    rp: &crate::compiler::resolve::ResolvedProgram,
) {
    match s {
        Stmt::PrintStr(_) => {}
        Stmt::PrintRawStr(_) => {}
        Stmt::PrintExpr(e) => {
            // Allow println concatenation: "str" + any + ... without flagging
            // arithmetic-on-non-integers errors. We skip strict typing for
            // additive chains that start with a string literal and just enforce
            // move/effect checks on the subexpressions.
            fn starts_with_str_concat(expr: &Expr) -> bool {
                match expr {
                    Expr::Str(_) => true,
                    Expr::Binary(l, crate::compiler::ast::BinOp::Add, _r) => {
                        starts_with_str_concat(l)
                    }
                    _ => false,
                }
            }

            if starts_with_str_concat(e) {
                // Only perform move/effect/concurrency checks; avoid type errors
                // from general '+' rules for non-integers.
                check_moves_in_expr(e, env, sf, reporter, current_pkg, rp);
            } else {
                let _ = type_of_expr(e, env, sf, reporter);
                check_moves_in_expr(e, env, sf, reporter, current_pkg, rp);
            }
        }
        Stmt::PrintRawExpr(e) => {
            // Same logic as PrintExpr but for raw print
            fn starts_with_str_concat(expr: &Expr) -> bool {
                match expr {
                    Expr::Str(_) => true,
                    Expr::Binary(l, crate::compiler::ast::BinOp::Add, _r) => {
                        starts_with_str_concat(l)
                    }
                    _ => false,
                }
            }

            if starts_with_str_concat(e) {
                check_moves_in_expr(e, env, sf, reporter, current_pkg, rp);
            } else {
                let _ = type_of_expr(e, env, sf, reporter);
                check_moves_in_expr(e, env, sf, reporter, current_pkg, rp);
            }
        }
        Stmt::If {
            cond,
            then_blk,
            else_blk,
        } => {
            let ct = type_of_expr(cond, env, sf, reporter);
            if !matches!(ct, Ty::Bool | Ty::Unknown) {
                reporter.emit(
                    Diagnostic::error("if condition must be bool").with_file(sf.path.clone()),
                );
            }
            // Definite assignment analysis: track initialization state across branches
            let before_if = env.init_env.clone();
            // Lifetime tracking: save state before if
            let lifetime_before = env.lifetime_env.clone();
            // Move state tracking: capture move states before if
            let move_states_before = env.capture_move_states();

            // Type-check then branch
            env.push();
            typecheck_block(then_blk, env, sf, reporter, current_pkg, rp);
            env.pop_with_lifetime_check(sf, reporter);
            let after_then = env.init_env.clone();
            let lifetime_after_then = env.lifetime_env.clone();
            let move_states_after_then = env.capture_move_states();

            if let Some(eb) = else_blk {
                // Reset to state before if, then check else branch
                env.init_env = before_if.clone();
                env.lifetime_env = lifetime_before.clone();
                env.restore_move_states(&move_states_before);
                env.push();
                typecheck_block(eb, env, sf, reporter, current_pkg, rp);
                env.pop_with_lifetime_check(sf, reporter);
                let after_else = env.init_env.clone();
                let lifetime_after_else = env.lifetime_env.clone();
                let move_states_after_else = env.capture_move_states();

                // Join: a variable is definitely initialized after if-else only if
                // it's initialized in BOTH branches
                env.init_env = after_then.join(&after_else);
                // Join lifetime environments - conservatively union borrows from both branches
                let (joined_lifetime, lifetime_errors) =
                    lifetime_after_then.join(&lifetime_after_else);
                env.lifetime_env = joined_lifetime;
                for err in lifetime_errors {
                    reporter.emit(Diagnostic::error(err.to_message()).with_file(sf.path.clone()));
                }
                // Join move states: if moved in one branch but not other, mark as conditionally moved
                let joined_move_states = Env::join_move_states(
                    &move_states_before,
                    &move_states_after_then,
                    &move_states_after_else,
                );
                env.restore_move_states(&joined_move_states);
            } else {
                // No else branch: join then-branch state with before-if state
                // (if-without-else can only add "possibly initialized", not "definitely")
                env.init_env = before_if.join(&after_then);
                // Join lifetime env - borrow might or might not have happened
                let (joined_lifetime, lifetime_errors) = lifetime_before.join(&lifetime_after_then);
                env.lifetime_env = joined_lifetime;
                for err in lifetime_errors {
                    reporter.emit(Diagnostic::error(err.to_message()).with_file(sf.path.clone()));
                }
                // Join move states: if moved in then branch, mark as conditionally moved
                let joined_move_states = Env::join_move_states_single_branch(
                    &move_states_before,
                    &move_states_after_then,
                );
                env.restore_move_states(&joined_move_states);
            }
        }
        Stmt::While { cond, body } => {
            let ct = type_of_expr(cond, env, sf, reporter);
            if !matches!(ct, Ty::Bool | Ty::Unknown) {
                reporter.emit(
                    Diagnostic::error("while condition must be bool").with_file(sf.path.clone()),
                );
            }
            check_moves_in_expr(cond, env, sf, reporter, current_pkg, rp);
            // Save lifetime state before loop for conservative join
            let lifetime_before = env.lifetime_env.clone();
            // Definite assignment: save initialization state before loop
            let init_before_loop = env.init_env.clone();
            // Enter loop region for region-based allocation tracking
            let loop_region_id = env.lifetime_env.enter_loop_region();
            // Track that we're in a loop for break/continue validation (empty string = unlabeled)
            env.push_label(String::new(), true);
            env.push();
            typecheck_block(body, env, sf, reporter, current_pkg, rp);
            env.pop_with_lifetime_check(sf, reporter);
            env.pop_label();
            // Capture initialization state after a single loop body execution
            let init_after_body = env.init_env.clone();
            let lifetime_after_body = env.lifetime_env.clone();
            // Exit loop region before joining
            env.lifetime_env.exit_loop_region(loop_region_id);
            // Join: borrows may or may not occur depending on loop iterations
            let (joined_lifetime, lifetime_errors) = lifetime_before.join(&lifetime_after_body);
            env.lifetime_env = joined_lifetime;
            for err in lifetime_errors {
                reporter.emit(Diagnostic::error(err.to_message()).with_file(sf.path.clone()));
            }
            // Definite assignment join: loop body may execute zero or more times.
            // A variable is definitely initialized after the loop only if it was
            // definitely initialized before the loop and remains so after a body
            // iteration on all paths.
            env.init_env = init_before_loop.join(&init_after_body);
        }
        Stmt::For {
            init,
            cond,
            step,
            body,
        } => {
            env.push();
            // Definite assignment: track initialization across init/body/step
            let _init_before_loop = env.init_env.clone();
            if let Some(i) = init {
                typecheck_stmt(i, env, sf, reporter, current_pkg, rp);
            }
            // Initialization state after executing the init clause (which always runs)
            let init_after_init = env.init_env.clone();
            // Save lifetime state after init (init always runs)
            let lifetime_before_iter = env.lifetime_env.clone();
            // Enter loop region for region-based allocation tracking
            let loop_region_id = env.lifetime_env.enter_loop_region();
            // Track that we're in a loop for break/continue validation
            env.push_label(String::new(), true);
            if let Some(c) = cond {
                let ct = type_of_expr(c, env, sf, reporter);
                if !matches!(ct, Ty::Bool | Ty::Unknown) {
                    reporter.emit(
                        Diagnostic::error("for condition must be bool").with_file(sf.path.clone()),
                    );
                }
                check_moves_in_expr(c, env, sf, reporter, current_pkg, rp);
            }
            // The loop body is a block scope (and per-iteration for borrows/locals).
            env.push();
            typecheck_block(body, env, sf, reporter, current_pkg, rp);
            env.pop_with_lifetime_check(sf, reporter);
            if let Some(st) = step {
                typecheck_stmt(st, env, sf, reporter, current_pkg, rp);
            }
            env.pop_label();
            let lifetime_after_iter = env.lifetime_env.clone();
            // Exit loop region before joining
            env.lifetime_env.exit_loop_region(loop_region_id);
            // Join: the loop body may execute zero or more times
            let (joined_lifetime, lifetime_errors) =
                lifetime_before_iter.join(&lifetime_after_iter);
            env.lifetime_env = joined_lifetime;
            for err in lifetime_errors {
                reporter.emit(Diagnostic::error(err.to_message()).with_file(sf.path.clone()));
            }
            // Capture initialization state after one full iteration (cond + body + step)
            let init_after_iter = env.init_env.clone();
            env.pop_with_lifetime_check(sf, reporter);
            // Definite assignment for for-loops:
            // - The init clause always executes.
            // - The body/step may execute zero or more times.
            // A variable is definitely initialized after the loop if it is
            // definitely initialized after init and remains so after a single
            // iteration on all paths.
            env.init_env = init_after_init.join(&init_after_iter);
        }
        Stmt::Switch {
            expr,
            cases,
            pattern_cases,
            default,
        } => {
            // Track that we're in a switch for break validation (false = not a loop)
            env.push_label(String::new(), false);
            let scrutinee_ty = type_of_expr(expr, env, sf, reporter);
            check_moves_in_expr(expr, env, sf, reporter, current_pkg, rp);
            // Definite assignment tracking for switch:
            // - With a default: all control-flow paths must go through one of the
            //   case/default blocks, so we join across all branches.
            // - Without default: there may be a path where no case matches; join
            //   branch state with the pre-switch state.
            let init_before_switch = env.init_env.clone();
            let mut branch_inits: Vec<InitEnv> = Vec::new();
            for (_, blk) in cases {
                // Reset init_env to pre-switch state for each case branch
                env.init_env = init_before_switch.clone();
                env.push();
                typecheck_block(blk, env, sf, reporter, current_pkg, rp);
                branch_inits.push(env.init_env.clone());
                env.pop_with_lifetime_check(sf, reporter);
            }
            // Handle pattern cases (enum pattern matching)
            // Collect patterns for exhaustiveness checking
            let mut exhaustiveness_patterns: Vec<exhaustiveness::Pat> = Vec::new();
            let mut is_exhaustive_match = false;

            for (pat, blk) in pattern_cases {
                // Reset init_env to pre-switch state for each pattern case branch
                env.init_env = init_before_switch.clone();
                env.push();
                // Type check the pattern and get bindings
                let bindings = typecheck_pattern(pat, &scrutinee_ty, env, sf, reporter);

                // Convert to exhaustiveness pattern
                exhaustiveness_patterns.push(exhaustiveness::ast_to_pat_with_scrutinee(
                    pat,
                    &scrutinee_ty,
                ));

                // Introduce bindings into scope
                for (name, ty) in bindings {
                    let needs_drop = env.needs_drop(&ty);
                    let drop_ty_name = env.drop_ty_name(&ty);
                    let decl_order = env.next_decl_order();
                    env.declare(
                        &name,
                        LocalInfo {
                            ty,
                            is_final: false,
                            initialized: true,
                            moved: false,
                            move_state: MoveState::Available,
                            num: None,
                            col_kind: None,
                            needs_drop,
                            drop_ty_name,
                            decl_order,
                        },
                    );
                    // Declare and mark as initialized in definite assignment analysis
                    env.init_env.declare_initialized(name.clone());
                }
                typecheck_block(blk, env, sf, reporter, current_pkg, rp);
                branch_inits.push(env.init_env.clone());
                env.pop_with_lifetime_check(sf, reporter);
            }

            // Exhaustiveness checking for pattern matching
            // Per spec: "enums (sealed by default), switch must be exhaustive"
            // Use the exhaustiveness checker for proper analysis
            if !pattern_cases.is_empty() && default.is_none() {
                let ctx = exhaustiveness::ExhaustivenessCtx {
                    enum_variants: &env.enum_variants,
                    current_pkg: env.current_pkg.clone(),
                };

                let result = exhaustiveness::check_exhaustiveness(
                    &exhaustiveness_patterns,
                    &scrutinee_ty,
                    &ctx,
                );

                // Report non-exhaustive matches
                if !result.is_exhaustive {
                    let witnesses: Vec<String> = result
                        .witnesses
                        .iter()
                        .map(|w| w.description.clone())
                        .collect();
                    reporter.emit(
                        Diagnostic::error(format!(
                            "non-exhaustive pattern match: missing cases {}. \
                            Add the missing cases or a 'default:' clause",
                            if witnesses.is_empty() {
                                "for some values".to_string()
                            } else {
                                witnesses.join(", ")
                            }
                        ))
                        .with_file(sf.path.clone()),
                    );
                } else {
                    // Mark as exhaustive for definite assignment
                    is_exhaustive_match = true;
                }

                // Report redundant patterns (warnings)
                for idx in result.redundant_patterns {
                    reporter.emit(
                        Diagnostic::warning(format!(
                            "unreachable pattern: case {} is covered by previous patterns",
                            idx + 1
                        ))
                        .with_file(sf.path.clone()),
                    );
                }
            }

            if let Some(db) = default {
                // Reset init_env to pre-switch state for default branch
                env.init_env = init_before_switch.clone();
                env.push();
                typecheck_block(db, env, sf, reporter, current_pkg, rp);
                branch_inits.push(env.init_env.clone());
                env.pop_with_lifetime_check(sf, reporter);
                // Exhaustive: all paths go through some case or default.
                if let Some(first) = branch_inits.first().cloned() {
                    let mut acc = first;
                    for b in branch_inits.iter().skip(1) {
                        acc.join_in_place(b);
                    }
                    env.init_env = acc;
                }
            } else if is_exhaustive_match && !branch_inits.is_empty() {
                // Pattern match is exhaustive: all paths go through some case.
                if let Some(first) = branch_inits.first().cloned() {
                    let mut acc = first;
                    for b in branch_inits.iter().skip(1) {
                        acc.join_in_place(b);
                    }
                    env.init_env = acc;
                }
            } else if !branch_inits.is_empty() {
                // Non-exhaustive: join pre-switch state with all case branches.
                let mut acc = branch_inits[0].clone();
                for b in branch_inits.iter().skip(1) {
                    acc.join_in_place(b);
                }
                env.init_env = init_before_switch.join(&acc);
            }
            env.pop_label();
        }
        Stmt::Try {
            try_blk,
            catches,
            finally_blk,
        } => {
            // === TRY/CATCH SAFETY RULES (scope.md §9) ===
            //
            // 1. Scope boundaries: values moved before throw are invalid in catch
            // 2. Exclusive borrows do not cross throw boundaries
            // 3. Deterministic drops: destructors run on unwind
            // 4. Finally constraints: cannot move values out, extend lifetimes
            // 5. Mutator effects: track mutations across try/catch
            // 6. catch(Error) only at boundary layers (warning)

            // Track effect system: enter try block
            env.effect_env.enter_try();

            // Check for exclusive borrows that would cross throw boundaries.
            // Exclusive borrows active before try cannot safely cross to catch handlers.
            let borrows_before_try: Vec<String> = env
                .excl_borrows
                .last()
                .map(|s| s.iter().cloned().collect())
                .unwrap_or_default();

            // P4-9: Track shared borrows before try block.
            // Shared borrows of values that get moved in try are invalidated in catch.
            let shared_borrows_before_try: HashSet<String> = env.get_shared_borrow_names();

            // Definite assignment: track initialization across try/catch/finally paths.
            // Analyze try in an isolated env to compute which outer locals were moved.
            let before = env.clone();
            let mut try_env = env.clone();
            try_env.push();
            typecheck_block(try_blk, &mut try_env, sf, reporter, current_pkg, rp);
            try_env.pop_with_lifetime_check(sf, reporter);
            let init_after_try = try_env.init_env.clone();

            // Collect names that became moved inside try compared to before.
            let mut moved_in_try: HashSet<String> = HashSet::new();
            for name in before.all_names() {
                if let (Some(bi), Some(ai)) = (before.get(&name), try_env.get(&name))
                    && !bi.moved
                    && ai.moved
                {
                    moved_in_try.insert(name);
                }
            }
            // Apply move effects to the outer env going forward.
            for name in &moved_in_try {
                if let Some(li) = env.get_mut(name) {
                    li.moved = true;
                }
            }

            // Safety Rule 2: Exclusive borrows do not cross throw boundaries.
            // If any exclusive borrows are still active after try block, they cannot
            // safely be used in catch handlers (the throw may have occurred while borrowed).
            for borrow_name in &borrows_before_try {
                if is_excl_borrowed(env, borrow_name) {
                    reporter.emit(
                        Diagnostic::warning(format!(
                            "exclusive borrow of '{}' may cross throw boundary; \
                            release the borrow before try or handle carefully in catch",
                            borrow_name
                        ))
                        .with_file(sf.path.clone()),
                    );
                }
            }

            // Safety Rule 7: UnwindSafe check for variables crossing throw boundaries.
            // Variables with interior mutability (Shared<T>, Watch<T>) or exclusive borrows
            // may have inconsistent state in catch handlers due to interrupted mutations.
            let mut non_unwind_safe_vars: Vec<(String, String)> = Vec::new();
            for name in before.all_names() {
                if let Some(li) = before.get(&name) {
                    if !li.moved && !is_unwind_safe(env, &li.ty) {
                        // Format the type name for the warning
                        let ty_name = format!("{:?}", li.ty);
                        non_unwind_safe_vars.push((name, ty_name));
                    }
                }
            }
            for (var_name, ty_name) in &non_unwind_safe_vars {
                reporter.emit(
                    Diagnostic::warning(format!(
                        "variable '{}' of type '{}' is not UnwindSafe; \
                        its state may be inconsistent in catch handlers due to interrupted mutations",
                        var_name, ty_name
                    ))
                    .with_file(sf.path.clone())
                    .with_suggestion(
                        "use Atomic<T> instead of Shared<T> for unwind-safe interior mutability, \
                        or avoid accessing this variable in catch handlers".to_string()
                    ),
                );
            }

            // P4-9: Identify shared borrows whose source was moved in try.
            // These borrows are invalidated in catch handlers.
            let invalidated_shared_borrows: HashSet<String> = shared_borrows_before_try
                .intersection(&moved_in_try)
                .cloned()
                .collect();

            // Warn about shared borrows that cross throw boundaries for moved values
            for borrow_name in &invalidated_shared_borrows {
                reporter.emit(
                    Diagnostic::warning(format!(
                        "shared borrow of '{}' may be invalidated by throw; \
                        the value was moved in the try block before exception",
                        borrow_name
                    ))
                    .with_file(sf.path.clone()),
                );
            }

            // Collect initialization states from catch blocks
            let mut catch_inits: Vec<InitEnv> = Vec::new();
            // Each catch runs in a scope where moved values in try are invalid.
            for cb in catches {
                // Safety Rule 6: catch(Error) only at boundary layers.
                // Catching the base Error type is discouraged except at program boundaries.
                if let Some(ref ty) = cb.ty {
                    let parts: Vec<String> = ty.path.iter().map(|i| i.0.clone()).collect();
                    if parts.len() == 1 && parts[0] == "Error" {
                        // Check if we're in a boundary context (main function or CLI handler)
                        let is_boundary = env.in_main
                            || env
                                .current_module
                                .as_ref()
                                .map(|m| {
                                    m.contains("Cli")
                                        || m.contains("Main")
                                        || m.contains("Entry")
                                        || m.contains("Handler")
                                })
                                .unwrap_or(false);
                        if !is_boundary {
                            reporter.emit(
                                Diagnostic::warning(
                                    "catching base 'Error' type is discouraged except at boundary layers \
                                    (CLI, task entrypoints, FFI shims); prefer catching specific exception types"
                                )
                                .with_file(sf.path.clone()),
                            );
                        }
                    }
                }

                let mut catch_env = env.clone();
                for name in &moved_in_try {
                    if let Some(li) = catch_env.get_mut(name) {
                        li.moved = true;
                    }
                }
                // P4-9: Clear shared borrows whose source was moved in try block.
                // These borrows are invalid in catch handlers because the source may have
                // been dropped before the exception was thrown.
                for name in &invalidated_shared_borrows {
                    catch_env.clear_shared_borrows_for(name);
                }
                catch_env.push();
                // Declare the catch variable if present
                if let Some(var) = &cb.var {
                    let ty = if let Some(ref np) = cb.ty {
                        map_name_to_ty(np)
                    } else {
                        Ty::Unknown
                    };
                    let needs_drop = catch_env.needs_drop(&ty);
                    let drop_ty_name = catch_env.drop_ty_name(&ty);
                    let decl_order = catch_env.next_decl_order();
                    catch_env.declare(
                        &var.0,
                        LocalInfo {
                            ty,
                            is_final: false,
                            initialized: true,
                            moved: false,
                            move_state: MoveState::Available,
                            num: None,
                            col_kind: None,
                            needs_drop,
                            drop_ty_name,
                            decl_order,
                        },
                    );
                    // Mark the catch variable as definitely initialized in the init_env
                    catch_env.init_env.declare_initialized(var.0.clone());
                }
                typecheck_block(&cb.blk, &mut catch_env, sf, reporter, current_pkg, rp);
                catch_env.pop_with_lifetime_check(sf, reporter);
                catch_inits.push(catch_env.init_env.clone());
            }
            // Aggregate catch initialization: join across all catches (if any)
            let init_after_catches = if let Some(first) = catch_inits.first().cloned() {
                let mut acc = first;
                for ci in catch_inits.iter().skip(1) {
                    acc.join_in_place(ci);
                }
                Some(acc)
            } else {
                None
            };

            // Track initialization effects from finally, if present.
            // Safety Rule 4: Finally constraints - cannot move values out or extend lifetimes.
            let mut init_after_finally: Option<InitEnv> = None;
            if let Some(fb) = finally_blk {
                let mut fin_env = env.clone();
                // Track which values are available before finally for move detection
                let available_before_finally: HashSet<String> = fin_env
                    .all_names()
                    .into_iter()
                    .filter(|n| {
                        fin_env
                            .get(n)
                            .map(|li| !li.moved && li.initialized)
                            .unwrap_or(false)
                    })
                    .collect();

                for name in &moved_in_try {
                    if let Some(li) = fin_env.get_mut(name) {
                        li.moved = true;
                    }
                }
                // P4-9: Clear shared borrows whose source was moved in try block.
                for name in &invalidated_shared_borrows {
                    fin_env.clear_shared_borrows_for(name);
                }
                // Mark that we're inside a finally block and track depth for labeled break/continue
                fin_env.in_finally = true;
                fin_env.finally_depth += 1;
                fin_env.push();
                typecheck_block(fb, &mut fin_env, sf, reporter, current_pkg, rp);
                fin_env.pop_with_lifetime_check(sf, reporter);
                fin_env.finally_depth -= 1;

                // Check for values moved out in finally (not allowed per spec)
                for name in &available_before_finally {
                    if let Some(li) = fin_env.get(name)
                        && li.moved
                        && !moved_in_try.contains(name)
                    {
                        reporter.emit(
                            Diagnostic::error(format!(
                                "finally block cannot move value '{}' out; \
                                values must remain available after finally completes",
                                name
                            ))
                            .with_file(sf.path.clone()),
                        );
                    }
                }

                init_after_finally = Some(fin_env.init_env.clone());
            }

            // Exit try block in effect system
            env.effect_env.exit_try();

            // Combine initialization from try, catches, and finally:
            // - Execution always goes through try; on exception, one of the catches
            //   (if present) may run, followed by finally (if present).
            // - For definite assignment, we conservatively join all reachable paths.
            let mut combined = init_after_try;
            if let Some(ci) = init_after_catches {
                combined.join_in_place(&ci);
            }
            if let Some(fi) = init_after_finally {
                combined.join_in_place(&fi);
            }
            // Only include paths that reach the statement end. Uncaught exceptions
            // do not reach here, so they should not affect definite assignment.
            env.init_env = combined;
        }
        Stmt::Assign { name, expr } => {
            let t = type_of_expr(expr, env, sf, reporter);

            // Effect system: record mutation effect
            env.effect_env.record_mutation();

            // Check for unsafe shared mutation
            if env.effect_env.shared_vars.contains(&name.0) {
                let safety = env.effect_env.check_mutation(&name.0);
                if !safety.is_safe() {
                    reporter
                        .emit(Diagnostic::warning(safety.describe()).with_file(sf.path.clone()));
                }
            }

            // Check for any active borrows using the lifetime environment.
            // Assignment invalidates borrows since it changes the value.
            if let Some(err) = env.lifetime_env.check_assign(&name.0) {
                reporter.emit(Diagnostic::error(err.to_message()).with_file(sf.path.clone()));
            }
            // Legacy check for exclusive borrows (for compatibility with borrowMut/release pattern)
            if is_excl_borrowed(env, &name.0) {
                reporter.emit(
                    Diagnostic::error(format!(
                        "cannot assign to '{}' while it is exclusively borrowed (inferred from a previous borrowMut({})); call release({}) before assigning",
                        name.0, name.0, name.0
                    ))
                    .with_file(sf.path.clone())
                    .with_suggestion(format!("call release({}) before assigning", name.0)),
                );
            }
            // P4-4/P4-5: Check that mutation is allowed (no shared borrows without exclusive)
            if let Err(msg) = env.check_mutation_allowed(&name.0) {
                reporter.emit(
                    Diagnostic::error(format!("cannot assign to '{}': {}", name.0, msg))
                        .with_file(sf.path.clone())
                        .with_suggestion(format!(
                            "use borrowMut({}) to get exclusive access before mutation",
                            name.0
                        )),
                );
            }
            if let Some(li) = env.get_mut(&name.0) {
                if li.is_final && li.initialized {
                    reporter.emit(
                        Diagnostic::error(format!(
                            "cannot reassign to final variable '{}'",
                            name.0
                        ))
                        .with_file(sf.path.clone()),
                    );
                } else if !same_type(&li.ty, &t) {
                    reporter.emit(
                        Diagnostic::error(format!(
                            "type mismatch in assignment to '{}': expected {:?}, found {:?}",
                            name.0, li.ty, t
                        ))
                        .with_file(sf.path.clone()),
                    );
                } else {
                    li.initialized = true;
                    // Fresh assignment restores ownership of the LHS binding.
                    li.moved = false;
                    li.move_state.reset();
                    // Mark as initialized in definite assignment analysis
                    env.init_env.initialize(&name.0);
                }
            } else {
                reporter.emit(
                    Diagnostic::error(format!("assignment to undeclared local '{}'", name.0))
                        .with_file(sf.path.clone()),
                );
            }
            // Any assignment updates what this local refers to; clear stale holder state.
            if let Some(local_lt) = env.lifetime_env.local_lifetimes.get_mut(&name.0) {
                local_lt.holds_borrow = None;
            }
            maybe_bind_provider_borrow_holder(&name.0, expr, env, current_pkg, rp);
            // Move semantics: assigning from a non-copy identifier moves it, unless assigning to self.
            if let Expr::Ident(src) = expr
                && src.0 != name.0
                && let Some(li_src) = env.get(&src.0)
                && !is_copy_ty(&li_src.ty, env)
            {
                consume_ident(&src.0, env, sf, reporter);
            } else {
                // Also scan RHS for call-argument moves.
                check_moves_in_expr(expr, env, sf, reporter, current_pkg, rp);
            }
        }
        Stmt::FieldAssign {
            object,
            field,
            expr,
        } => {
            // Determine the struct field type if possible and type-check the RHS.
            let rhs_ty = type_of_expr(expr, env, sf, reporter);
            // Check object exists if it's a simple identifier
            if let Expr::Ident(id) = object {
                if env.get(&id.0).is_none() {
                    reporter.emit(
                        Diagnostic::error(format!(
                            "assignment to field of undeclared local '{}'",
                            id.0
                        ))
                        .with_file(sf.path.clone()),
                    );
                }

                // P4-4/P4-5: Check that mutation is allowed (no shared borrows without exclusive)
                if let Err(msg) = env.check_mutation_allowed(&id.0) {
                    reporter.emit(
                        Diagnostic::error(format!(
                            "cannot assign to field '{}' of '{}': {}",
                            field.0, id.0, msg
                        ))
                        .with_file(sf.path.clone())
                        .with_suggestion(format!(
                            "use borrowMut({}) to get exclusive access before field mutation",
                            id.0
                        )),
                    );
                }
            }
            // Infer object type and match field typing when known (avoid triggering init diagnostics for simple ident)
            let named_path_opt: Option<Vec<String>> = if let Expr::Ident(id) = object {
                env.get(&id.0).and_then(|li| match &li.ty {
                    Ty::Named(p) => Some(p.clone()),
                    _ => None,
                })
            } else {
                match type_of_expr(object, env, sf, reporter) {
                    Ty::Named(p) => Some(p),
                    _ => None,
                }
            };
            if let Some(ref path) = named_path_opt {
                if !path.is_empty() {
                    let (pkg, sname) = if path.len() == 1 {
                        (env.current_pkg.clone().unwrap_or_default(), path[0].clone())
                    } else {
                        (
                            path[..path.len() - 1].join("."),
                            path.last().unwrap().clone(),
                        )
                    };
                    if let Some(fields) = env.struct_fields.get(&(pkg.clone(), sname.clone())) {
                        if let Some((fty_np, field_vis, is_final, _default)) = fields.get(&field.0)
                        {
                            // Check field visibility
                            let current_pkg = env.current_pkg.clone().unwrap_or_default();
                            let can_access = match field_vis {
                                AS::Visibility::Public => true,
                                AS::Visibility::Internal => {
                                    fn top_package_segment(p: &str) -> &str {
                                        p.split('.').next().unwrap_or(p)
                                    }
                                    top_package_segment(&pkg) == top_package_segment(&current_pkg)
                                }
                                AS::Visibility::Private | AS::Visibility::Default => {
                                    pkg == current_pkg
                                }
                            };

                            if !can_access {
                                let vis_str = match field_vis {
                                    AS::Visibility::Public => "public",
                                    AS::Visibility::Internal => "internal",
                                    AS::Visibility::Private => "private",
                                    AS::Visibility::Default => "private",
                                };
                                reporter.emit(
                                    Diagnostic::error(format!(
                                        "field '{}' of struct {} is {} and not accessible from package {}",
                                        field.0, sname, vis_str, current_pkg
                                    ))
                                    .with_file(sf.path.clone()),
                                );
                            }

                            // Check if field is final (immutable)
                            if *is_final {
                                reporter.emit(
                                    Diagnostic::error(format!(
                                        "cannot assign to final field '{}.{}'",
                                        sname, field.0
                                    ))
                                    .with_file(sf.path.clone()),
                                );
                            }

                            let mut fty = map_name_to_ty(fty_np);
                            if let Ty::Named(ref mut p) = fty {
                                qualify_named_with_imports(p, env);
                            }
                            fty = resolve_alias_ty(fty, env);
                            if !same_type(&fty, &rhs_ty) {
                                reporter.emit(
                                    Diagnostic::error(format!(
                                        "type mismatch for field '{}.{}': expected {:?}, found {:?}",
                                        sname, field.0, fty, rhs_ty
                                    ))
                                    .with_file(sf.path.clone()),
                                );
                            }
                        } else {
                            reporter.emit(
                                Diagnostic::error(format!(
                                    "unknown field '{}' in struct {}",
                                    field.0, sname
                                ))
                                .with_file(sf.path.clone()),
                            );
                        }
                    }
                }
            }
            // Escape analysis: if the RHS is an identifier being assigned to a field,
            // it may escape if the object escapes. Conservative: mark as escaping via field.
            if let Expr::Ident(rhs_id) = expr {
                // If the object we're assigning to might escape (e.g., it's a parameter
                // or itself escapes), then the RHS value also escapes via field storage
                let object_may_escape = if let Expr::Ident(obj_id) = object {
                    // Check if the object escapes or is a function parameter (depth 0)
                    env.lifetime_env.does_escape(&obj_id.0)
                        || env.lifetime_env.is_parameter(&obj_id.0)
                } else {
                    // Complex object expression - conservatively assume it may escape
                    true
                };
                if object_may_escape {
                    env.lifetime_env.mark_escape_field(&rhs_id.0);
                }
            }

            // Provider borrow validation: provider fields cannot hold borrows to stack values.
            // Providers outlive function scope, so storing a borrow would create a dangling reference.
            if let Expr::Ident(obj_id) = object {
                // Get the object's type to check if it's a provider
                if let Some(obj_info) = env.get(&obj_id.0) {
                    if let Ty::Named(ref path) = obj_info.ty {
                        let (pkg, _type_name) = if path.len() == 1 {
                            (env.current_pkg.clone().unwrap_or_default(), path[0].clone())
                        } else {
                            (
                                path[..path.len() - 1].join("."),
                                path.last().unwrap().clone(),
                            )
                        };
                        // Check if this type is a provider
                        if let Some(ResolvedKind::Provider) = lookup_symbol_kind(rp, &pkg, path) {
                            let provider_type_name =
                                path.last().cloned().unwrap_or_else(|| obj_id.0.clone());
                            env.lifetime_env
                                .invalidate_provider_borrows(&provider_type_name);
                            // This is a provider - check if RHS holds a borrow
                            if let Expr::Ident(rhs_id) = expr {
                                if let Some(borrow_info) =
                                    env.lifetime_env.get_local_borrow_info(&rhs_id.0)
                                {
                                    // The RHS holds a borrow - providers can't store borrows to locals
                                    // Only report if the borrow is from a local (function-scoped)
                                    if !matches!(
                                        borrow_info.origin,
                                        lifetime::BorrowOrigin::Provider(_)
                                    ) {
                                        reporter.emit(
                                            Diagnostic::error(
                                                lifetime::LifetimeError::ProviderHoldsBorrow {
                                                    provider_name: provider_type_name.clone(),
                                                    field_name: field.0.clone(),
                                                    borrow_holder: rhs_id.0.clone(),
                                                    origin: borrow_info.origin.clone(),
                                                    mode: borrow_info.mode,
                                                }
                                                .to_message(),
                                            )
                                            .with_file(sf.path.clone()),
                                        );
                                    }
                                }
                            }
                            // Mark any RHS identifier as escaping via provider
                            if let Expr::Ident(rhs_id) = expr {
                                env.lifetime_env.mark_escape_provider(&rhs_id.0);
                            }
                        }
                    }
                }
            }

            // Effect system: record mutation effect and validate shared state access
            env.effect_env.record_mutation();

            // Check for shared field mutation safety
            if let Expr::Ident(obj_id) = object {
                if let Some(obj_info) = env.get(&obj_id.0) {
                    if let Ty::Named(ref path) = obj_info.ty {
                        // Check if this is a shared context (provider or shared field)
                        let is_shared = env.effect_env.shared_vars.contains(&obj_id.0);
                        if is_shared {
                            // Check if the field type is self-guarding
                            let is_self_guarding = if let Some(named_path) = &named_path_opt {
                                if let Some((pkg, sname)) = if named_path.len() == 1 {
                                    Some((
                                        env.current_pkg.clone().unwrap_or_default(),
                                        named_path[0].clone(),
                                    ))
                                } else {
                                    Some((
                                        named_path[..named_path.len() - 1].join("."),
                                        named_path.last().unwrap().clone(),
                                    ))
                                } {
                                    if let Some(fields) = env.struct_fields.get(&(pkg, sname)) {
                                        if let Some((fty_np, _, _, _)) = fields.get(&field.0) {
                                            let type_name = fty_np
                                                .path
                                                .last()
                                                .map(|i| i.0.as_str())
                                                .unwrap_or("");
                                            effects::is_self_guarding_type(type_name)
                                        } else {
                                            false
                                        }
                                    } else {
                                        false
                                    }
                                } else {
                                    false
                                }
                            } else {
                                false
                            };

                            if !is_self_guarding {
                                // Unsafe shared mutation
                                let _ = path; // silence unused warning
                                reporter.emit(
                                    Diagnostic::warning(format!(
                                        "mutation of shared field '{}.{}' should use a thread-safe wrapper (Atomic<T>, Shared<T>) or capability-guarded access",
                                        obj_id.0, field.0
                                    ))
                                    .with_file(sf.path.clone()),
                                );
                            }
                        }
                    }
                }
            }

            check_moves_in_expr(expr, env, sf, reporter, current_pkg, rp);
        }
        Stmt::AssignOp { name, op, expr } => {
            use AS::AssignOp as AO;
            let rhs_ty = type_of_expr(expr, env, sf, reporter);
            // Check for any active borrows using the lifetime environment.
            // Compound assignment reads and writes the value, invalidating borrows.
            if let Some(err) = env.lifetime_env.check_assign(&name.0) {
                reporter.emit(Diagnostic::error(err.to_message()).with_file(sf.path.clone()));
            }
            // Legacy check for exclusive borrows (for compatibility with borrowMut/release pattern)
            if is_excl_borrowed(env, &name.0) {
                reporter.emit(
                    Diagnostic::error(format!(
                        "cannot assign to '{}' while it is exclusively borrowed",
                        name.0
                    ))
                    .with_file(sf.path.clone())
                    .with_suggestion(format!("call release({}) before assigning", name.0)),
                );
            }
            // P4-4/P4-5: Check that mutation is allowed (no shared borrows without exclusive)
            if let Err(msg) = env.check_mutation_allowed(&name.0) {
                reporter.emit(
                    Diagnostic::error(format!(
                        "cannot apply compound assignment to '{}': {}",
                        name.0, msg
                    ))
                    .with_file(sf.path.clone())
                    .with_suggestion(format!(
                        "use borrowMut({}) to get exclusive access before mutation",
                        name.0
                    )),
                );
            }
            if let Some(li) = env.get_mut(&name.0) {
                if li.is_final && li.initialized {
                    reporter.emit(
                        Diagnostic::error(format!(
                            "cannot assign-op to final variable '{}'",
                            name.0
                        ))
                        .with_file(sf.path.clone()),
                    );
                }
                // Validate types based on operator kind
                let lhs_ty = &li.ty;
                let type_ok = match op {
                    // += supports Int, Float, and String (concatenation)
                    AO::Add => {
                        let is_numeric = matches!(lhs_ty, Ty::Int | Ty::Float | Ty::Unknown)
                            && matches!(rhs_ty, Ty::Int | Ty::Float | Ty::Unknown);
                        let is_string = matches!(lhs_ty, Ty::String | Ty::Unknown)
                            && matches!(rhs_ty, Ty::String | Ty::Unknown);
                        if !is_numeric && !is_string {
                            reporter.emit(
                                Diagnostic::error(
                                    "'+=' requires numeric (Int/Float) or String operands",
                                )
                                .with_file(sf.path.clone()),
                            );
                            false
                        } else {
                            true
                        }
                    }
                    // -=, *=, /=, %= support Int and Float
                    AO::Sub | AO::Mul | AO::Div | AO::Mod => {
                        let is_numeric = matches!(lhs_ty, Ty::Int | Ty::Float | Ty::Unknown)
                            && matches!(rhs_ty, Ty::Int | Ty::Float | Ty::Unknown);
                        if !is_numeric {
                            let op_str = match op {
                                AO::Sub => "-=",
                                AO::Mul => "*=",
                                AO::Div => "/=",
                                AO::Mod => "%=",
                                _ => "?=",
                            };
                            reporter.emit(
                                Diagnostic::error(format!(
                                    "'{}' requires numeric (Int/Float) operands",
                                    op_str
                                ))
                                .with_file(sf.path.clone()),
                            );
                            false
                        } else {
                            true
                        }
                    }
                    // <<=, >>=, &=, |=, ^= require Int only (bitwise operations)
                    AO::Shl | AO::Shr | AO::And | AO::Or | AO::Xor => {
                        let is_int = matches!(lhs_ty, Ty::Int | Ty::Unknown)
                            && matches!(rhs_ty, Ty::Int | Ty::Unknown);
                        if !is_int {
                            let op_str = match op {
                                AO::Shl => "<<=",
                                AO::Shr => ">>=",
                                AO::And => "&=",
                                AO::Or => "|=",
                                AO::Xor => "^=",
                                _ => "?=",
                            };
                            reporter.emit(
                                Diagnostic::error(format!(
                                    "'{}' requires integer operands",
                                    op_str
                                ))
                                .with_file(sf.path.clone()),
                            );
                            false
                        } else {
                            true
                        }
                    }
                };
                if type_ok {
                    li.initialized = true;
                    // Mark as initialized in definite assignment analysis
                    env.init_env.initialize(&name.0);
                }
            } else {
                reporter.emit(
                    Diagnostic::error(format!(
                        "compound assignment to undeclared local '{}'",
                        name.0
                    ))
                    .with_file(sf.path.clone()),
                );
            }
            // RHS may include calls; scan for moves on call args.
            check_moves_in_expr(expr, env, sf, reporter, current_pkg, rp);
        }
        Stmt::VarDecl {
            is_final,
            is_shared: _,
            ty,
            generics,
            fn_params,
            name,
            init,
        } => {
            let mut dty = map_name_to_ty(ty);

            // Handle type inference with 'var' keyword
            let is_var_inferred = matches!(&dty, Ty::Named(p) if p.len() == 1 && p[0] == "var");
            if is_var_inferred {
                if let Some(init_e) = init {
                    // Infer type from the initializer expression
                    dty = type_of_expr(init_e, env, sf, reporter);
                } else {
                    reporter.emit(
                        Diagnostic::error(format!(
                            "variable '{}' declared with 'var' must have an initializer for type inference",
                            name.0
                        ))
                        .with_file(sf.path.clone()),
                    );
                    dty = Ty::Unknown;
                }
            }

            // Qualify simple Named types using imports (e.g., Logger -> log.Logger; List -> list.List)
            if let Ty::Named(ref mut p) = dty {
                qualify_named_with_imports(p, env);
            }
            dty = resolve_alias_ty(dty, env);
            let col_kind = match last_segment_of_named(&dty) {
                Some("List") => generics.get(0).map(|g| {
                    let mut t = map_name_to_ty(g);
                    if let Ty::Named(ref mut pth) = t {
                        qualify_named_with_imports(pth, env);
                    }
                    t = resolve_alias_ty(t, env);
                    CollectionKind::List(Box::new(t))
                }),
                Some("Map") => {
                    let kt = generics.get(0).map(|g| {
                        let mut t = map_name_to_ty(g);
                        if let Ty::Named(ref mut pth) = t {
                            qualify_named_with_imports(pth, env);
                        }
                        resolve_alias_ty(t, env)
                    });
                    let vt = generics.get(1).map(|g| {
                        let mut t = map_name_to_ty(g);
                        if let Ty::Named(ref mut pth) = t {
                            qualify_named_with_imports(pth, env);
                        }
                        resolve_alias_ty(t, env)
                    });
                    match (kt, vt) {
                        (Some(k), Some(v)) => Some(CollectionKind::Map(Box::new(k), Box::new(v))),
                        _ => None,
                    }
                }
                _ => None,
            };
            // Detect function type: Fn<Ret>(ParamTypes...)
            let dty = match last_segment_of_named(&dty) {
                Some("Fn") => {
                    // First generic is return type; parameters from fn_params
                    let ret_ty = generics
                        .get(0)
                        .map(|g| {
                            let mut t = map_name_to_ty(g);
                            if let Ty::Named(ref mut pth) = t {
                                qualify_named_with_imports(pth, env);
                            }
                            resolve_alias_ty(t, env)
                        })
                        .unwrap_or(Ty::Unknown);
                    let mut pts: Vec<Ty> = Vec::new();
                    for np in fn_params {
                        let mut t = map_name_to_ty(np);
                        if let Ty::Named(ref mut pth) = t {
                            qualify_named_with_imports(pth, env);
                        }
                        t = resolve_alias_ty(t, env);
                        pts.push(t);
                    }
                    Ty::Function(pts, Box::new(ret_ty))
                }
                _ => dty,
            };
            let needs_drop = env.needs_drop(&dty);
            let drop_ty_name = env.drop_ty_name(&dty);
            let decl_order = env.next_decl_order();
            let mut info = LocalInfo {
                ty: dty.clone(),
                is_final: *is_final,
                initialized: false,
                moved: false,
                move_state: MoveState::Available,
                num: numty_from_namepath(ty),
                col_kind,
                needs_drop,
                drop_ty_name,
                decl_order,
            };
            // Final variables MUST have an initializer at declaration time
            if *is_final && init.is_none() {
                reporter.emit(
                    Diagnostic::error(format!(
                        "final variable '{}' must be initialized at declaration",
                        name.0
                    ))
                    .with_file(sf.path.clone()),
                );
            }
            if let Some(init_e) = init {
                // Special handling for function literal initializer matched against function type
                if let (Ty::Function(exp_params, exp_ret), Expr::FnLiteral(params, body)) =
                    (&dty, init_e)
                {
                    // Check param arity and types
                    if params.len() != exp_params.len() {
                        reporter.emit(
                            Diagnostic::error(format!(
                                "lambda parameter count mismatch: expected {}, found {}",
                                exp_params.len(),
                                params.len()
                            ))
                            .with_file(sf.path.clone()),
                        );
                    } else {
                        // Typecheck lambda body under a fresh env with expected return type and declared params
                        let mut inner = env.clone();
                        inner.push();
                        inner.expected_ret = (*exp_ret.as_ref()).clone();
                        for (decl, et) in params.iter().zip(exp_params.iter()) {
                            let mut pty = map_name_to_ty(&decl.ty);
                            if let Ty::Named(ref mut pth) = pty {
                                qualify_named_with_imports(pth, &inner);
                            }
                            pty = resolve_alias_ty(pty, &inner);
                            if !same_type(&pty, et) {
                                reporter.emit(
                                    Diagnostic::error(format!(
                                        "lambda parameter type mismatch: expected {:?}, found {:?}",
                                        et, pty
                                    ))
                                    .with_file(sf.path.clone()),
                                );
                            }
                            let needs_drop = inner.needs_drop(et);
                            let drop_ty_name = inner.drop_ty_name(et);
                            let decl_order = inner.next_decl_order();
                            inner.declare(
                                &decl.name.0,
                                LocalInfo {
                                    ty: et.clone(),
                                    is_final: false,
                                    initialized: true,
                                    moved: false,
                                    move_state: MoveState::Available,
                                    num: None,
                                    col_kind: None,
                                    needs_drop,
                                    drop_ty_name,
                                    decl_order,
                                },
                            );
                            // Mark lambda parameter as definitely initialized
                            inner.init_env.declare_initialized(decl.name.0.clone());
                        }
                        typecheck_block(body, &mut inner, sf, reporter, current_pkg, rp);
                        // Enforce capture moves for non-copy values at lambda creation
                        let param_names: std::collections::HashSet<String> =
                            params.iter().map(|p| p.name.0.clone()).collect();
                        let caps = collect_lambda_captures(body, params);
                        for name in &caps {
                            if param_names.contains(name) {
                                continue;
                            }
                            if let Some(li) = env.get(name) {
                                if !is_copy_ty(&li.ty, env) {
                                    consume_ident(name, env, sf, reporter);
                                }
                            }
                        }

                        // Closure capture borrow validation
                        for name in &caps {
                            if param_names.contains(name) {
                                continue;
                            }
                            if let Some(borrow_info) = env.lifetime_env.get_local_borrow_info(name)
                            {
                                let origin_desc = match &borrow_info.origin {
                                    lifetime::BorrowOrigin::Local(src) => format!("'{}'", src),
                                    lifetime::BorrowOrigin::Param(src) => {
                                        format!("parameter '{}'", src)
                                    }
                                    lifetime::BorrowOrigin::Field(obj, field_path) => {
                                        format!("field '{}.{}'", obj, field_path.join("."))
                                    }
                                    lifetime::BorrowOrigin::Provider(prov) => {
                                        format!("provider '{}'", prov)
                                    }
                                    lifetime::BorrowOrigin::Unknown => "unknown source".to_string(),
                                };
                                let mode_desc = match borrow_info.mode {
                                    lifetime::BorrowMode::Shared => "shared",
                                    lifetime::BorrowMode::Exclusive => "exclusive",
                                };
                                reporter.emit(
                                    Diagnostic::error(format!(
                                        "closure captures '{}' which holds a {} borrow of {}; \
                                         captured borrows may outlive their source",
                                        name, mode_desc, origin_desc
                                    ))
                                    .with_file(sf.path.clone()),
                                );
                            }
                            // Mark as escaping via closure
                            env.lifetime_env.mark_escape_closure(name);
                        }

                        inner.pop_with_lifetime_check(sf, reporter);
                        info.initialized = true;
                    }
                } else {
                    // Special handling for struct initialization via MapLit
                    // Syntax: StructName { field1: value1, field2: value2 }
                    // Or with spread: StructName { ..existing, field1: value1 }
                    if let Expr::MapLit { pairs, spread } = init_e {
                        if let Ty::Named(struct_path) = &dty {
                            // Determine the (package, struct name) for this type
                            let (pkg, sname) = if struct_path.len() == 1 {
                                (
                                    env.current_pkg.clone().unwrap_or_default(),
                                    struct_path[0].clone(),
                                )
                            } else {
                                (
                                    struct_path[..struct_path.len() - 1].join("."),
                                    struct_path.last().unwrap().clone(),
                                )
                            };
                            // Check if this is a known struct
                            if let Some(fields) =
                                env.struct_fields.get(&(pkg.clone(), sname.clone()))
                            {
                                // Validate spread expression if present
                                if let Some(spread_expr) = spread {
                                    let spread_ty = type_of_expr(spread_expr, env, sf, reporter);
                                    // Spread must be the same struct type
                                    if !same_type(&dty, &spread_ty) {
                                        reporter.emit(
                                            Diagnostic::error(format!(
                                                "spread expression type mismatch: expected {:?}, found {:?}",
                                                dty, spread_ty
                                            ))
                                            .with_file(sf.path.clone()),
                                        );
                                    }
                                }

                                // This is struct initialization - validate fields
                                let has_spread = spread.is_some();
                                let mut provided_fields: std::collections::HashSet<String> =
                                    std::collections::HashSet::new();

                                for (key_expr, value_expr) in pairs {
                                    // Field name must be an identifier
                                    if let Expr::Ident(field_name) = key_expr {
                                        let field_name_str = &field_name.0;

                                        // Check for duplicate fields
                                        if provided_fields.contains(field_name_str) {
                                            reporter.emit(
                                                Diagnostic::error(format!(
                                                    "duplicate field '{}' in struct literal {}",
                                                    field_name_str, sname
                                                ))
                                                .with_file(sf.path.clone()),
                                            );
                                        } else {
                                            provided_fields.insert(field_name_str.clone());
                                        }

                                        // Check if field exists and get its metadata
                                        if let Some((
                                            field_ty_np,
                                            _field_vis,
                                            _is_final,
                                            _default,
                                        )) = fields.get(field_name_str)
                                        {
                                            // Note: We DON'T check visibility or final here
                                            // Struct initialization via literals is always allowed to set all fields
                                            // (this is the "constructor" equivalent)

                                            // Type-check the value expression
                                            let value_ty =
                                                type_of_expr(value_expr, env, sf, reporter);
                                            let mut expected_ty = map_name_to_ty(field_ty_np);
                                            if let Ty::Named(ref mut pth) = expected_ty {
                                                qualify_named_with_imports(pth, env);
                                            }
                                            expected_ty = resolve_alias_ty(expected_ty, env);

                                            if !same_type(&expected_ty, &value_ty) {
                                                reporter.emit(
                                                    Diagnostic::error(format!(
                                                        "type mismatch for field '{}' in struct {}: expected {:?}, found {:?}",
                                                        field_name_str, sname, expected_ty, value_ty
                                                    ))
                                                    .with_file(sf.path.clone()),
                                                );
                                            }
                                        } else {
                                            reporter.emit(
                                                Diagnostic::error(format!(
                                                    "unknown field '{}' in struct {}",
                                                    field_name_str, sname
                                                ))
                                                .with_file(sf.path.clone()),
                                            );
                                        }
                                    } else {
                                        reporter.emit(
                                            Diagnostic::error(
                                                "struct field names must be identifiers",
                                            )
                                            .with_file(sf.path.clone()),
                                        );
                                    }
                                }

                                // Check for missing required fields (only if no spread)
                                if !has_spread {
                                    for (field_name, (_, _, _, default)) in fields.iter() {
                                        // Field is required if it has no default value
                                        if default.is_none()
                                            && !provided_fields.contains(field_name)
                                        {
                                            reporter.emit(
                                                Diagnostic::error(format!(
                                                    "missing required field '{}' in struct literal {}",
                                                    field_name, sname
                                                ))
                                                .with_file(sf.path.clone()),
                                            );
                                        }
                                    }
                                }

                                info.initialized = true;
                            } else {
                                // Not a known struct - treat as regular map
                                let it = type_of_expr(init_e, env, sf, reporter);
                                if !same_type(&dty, &it) {
                                    reporter.emit(
                                        Diagnostic::error(format!(
                                            "type mismatch in variable '{}': expected {:?}, found {:?}",
                                            name.0, dty, it
                                        ))
                                        .with_file(sf.path.clone()),
                                    );
                                } else {
                                    info.initialized = true;
                                }
                            }
                        } else {
                            // Not a Named type - treat as regular map
                            let it = type_of_expr(init_e, env, sf, reporter);
                            if !same_type(&dty, &it) {
                                reporter.emit(
                                    Diagnostic::error(format!(
                                        "type mismatch in variable '{}': expected {:?}, found {:?}",
                                        name.0, dty, it
                                    ))
                                    .with_file(sf.path.clone()),
                                );
                            } else {
                                info.initialized = true;
                            }
                        }
                    } else {
                        // Not a MapLit - regular type checking
                        let it = type_of_expr(init_e, env, sf, reporter);
                        if !same_type(&dty, &it) {
                            reporter.emit(
                                Diagnostic::error(format!(
                                    "type mismatch in variable '{}': expected {:?}, found {:?}",
                                    name.0, dty, it
                                ))
                                .with_file(sf.path.clone()),
                            );
                        } else {
                            info.initialized = true;
                        }
                    }
                }
                // Move-on-init from identifier of non-copy type
                if let Expr::Ident(src) = init_e
                    && let Some(li_src) = env.get(&src.0)
                    && !is_copy_ty(&li_src.ty, env)
                {
                    consume_ident(&src.0, env, sf, reporter);
                } else {
                    // Also scan initializer for call-argument moves.
                    check_moves_in_expr(init_e, env, sf, reporter, current_pkg, rp);
                }
            }
            // Final semantics enforced:
            // 1. Final variables must have initializer (checked above)
            // 2. Reassignment to final variables is blocked in Stmt::Assign handling
            env.declare(&name.0, info.clone());
            // Track initialization state in definite assignment analysis
            if info.initialized {
                env.init_env.declare_initialized(name.0.clone());
            } else {
                env.init_env.declare(name.0.clone());
            }
            if let Some(init_e) = init {
                maybe_bind_provider_borrow_holder(&name.0, init_e, env, current_pkg, rp);
            }

            // Effect system: track self-guarding types and shared status
            if let Ty::Named(ref path) = dty {
                let type_name = path.last().map(|s| s.as_str()).unwrap_or("");
                // Mark variables with self-guarding types
                if effects::is_self_guarding_type(type_name) {
                    env.effect_env.mark_self_guarding(&name.0);
                }
                // Mark thread-local by default (shared status comes from context)
                env.effect_env.mark_thread_local(&name.0);
            }
        }
        Stmt::Break(label) => {
            match label {
                Some(lbl) => {
                    // Labeled break - validate label exists and targets loop or switch
                    match env.find_label(&lbl.0) {
                        Some((_, label_depth)) => {
                            // Label exists - check finally block constraints
                            // If label was declared outside finally but we're inside finally, it's invalid
                            if env.finally_depth > label_depth {
                                reporter.emit(
                                    Diagnostic::error(format!(
                                        "break to label '{}' crosses finally block boundary; this is not allowed",
                                        lbl.0
                                    ))
                                    .with_file(sf.path.clone()),
                                );
                            }
                        }
                        None => {
                            reporter.emit(
                                Diagnostic::error(format!(
                                    "undefined label '{}' in break statement",
                                    lbl.0
                                ))
                                .with_file(sf.path.clone()),
                            );
                        }
                    }
                }
                None => {
                    // Unlabeled break - must be inside a loop or switch
                    if !env.in_loop_or_switch() {
                        reporter.emit(
                            Diagnostic::error("break statement not inside loop or switch")
                                .with_file(sf.path.clone()),
                        );
                    } else if env.in_finally {
                        reporter.emit(
                            Diagnostic::error(
                                "break in finally block may skip pending return or throw; this is not allowed",
                            )
                            .with_file(sf.path.clone()),
                        );
                    }
                }
            }
        }
        Stmt::Continue(label) => {
            match label {
                Some(lbl) => {
                    // Labeled continue - validate label exists and targets a loop (not switch)
                    match env.find_label(&lbl.0) {
                        Some((is_loop, label_depth)) => {
                            if !is_loop {
                                reporter.emit(
                                    Diagnostic::error(format!(
                                        "continue to label '{}' targets a switch statement; continue can only target loops",
                                        lbl.0
                                    ))
                                    .with_file(sf.path.clone()),
                                );
                            } else if env.finally_depth > label_depth {
                                // Label was declared outside finally but we're inside finally
                                reporter.emit(
                                    Diagnostic::error(format!(
                                        "continue to label '{}' crosses finally block boundary; this is not allowed",
                                        lbl.0
                                    ))
                                    .with_file(sf.path.clone()),
                                );
                            }
                        }
                        None => {
                            reporter.emit(
                                Diagnostic::error(format!(
                                    "undefined label '{}' in continue statement",
                                    lbl.0
                                ))
                                .with_file(sf.path.clone()),
                            );
                        }
                    }
                }
                None => {
                    // Unlabeled continue - must be inside a loop (not switch)
                    if !env.in_loop() {
                        reporter.emit(
                            Diagnostic::error("continue statement not inside loop")
                                .with_file(sf.path.clone()),
                        );
                    } else if env.in_finally {
                        reporter.emit(
                            Diagnostic::error(
                                "continue in finally block may skip pending return or throw; this is not allowed",
                            )
                            .with_file(sf.path.clone()),
                        );
                    }
                }
            }
        }
        Stmt::Return(val) => {
            // Check if we're inside a finally block - return in finally overrides pending return/throw
            if env.in_finally {
                reporter.emit(
                    Diagnostic::error(
                        "return statement in finally block would override pending return or throw; this is not allowed (per spec: finally cannot move values out)",
                    )
                    .with_file(sf.path.clone()),
                );
            }
            let expected_ret = env.expected_ret.clone();
            match (expected_ret, val) {
                (Ty::Void, None) => { /* ok */ }
                (Ty::Void, Some(e)) => {
                    let _ = type_of_expr(e, env, sf, reporter);
                    // Apply move semantics for return expression (may move non-copy values)
                    check_moves_in_expr(e, env, sf, reporter, current_pkg, rp);
                    reporter.emit(
                        Diagnostic::error(
                            "cannot return a value from a function with 'void' return type",
                        )
                        .with_file(sf.path.clone()),
                    );
                }
                (t_expected, None) => {
                    reporter.emit(
                        Diagnostic::error(format!("missing return value of type {:?}", t_expected))
                            .with_file(sf.path.clone()),
                    );
                }
                (t_expected, Some(e)) => {
                    // Special handling for struct literals in return statements
                    // When returning a MapLit to a Named struct type, treat it as struct initialization
                    if let Expr::MapLit { pairs, spread } = e {
                        if let Ty::Named(struct_path) = &t_expected {
                            // Determine the (package, struct name) for this type
                            let (pkg, sname) = if struct_path.len() == 1 {
                                (
                                    env.current_pkg.clone().unwrap_or_default(),
                                    struct_path[0].clone(),
                                )
                            } else {
                                (
                                    struct_path[..struct_path.len() - 1].join("."),
                                    struct_path.last().unwrap().clone(),
                                )
                            };
                            // Check if this is a known struct
                            if let Some(fields) =
                                env.struct_fields.get(&(pkg.clone(), sname.clone()))
                            {
                                // Validate spread expression if present
                                if let Some(spread_expr) = spread {
                                    let spread_ty = type_of_expr(spread_expr, env, sf, reporter);
                                    if !same_type(&t_expected, &spread_ty) {
                                        reporter.emit(
                                            Diagnostic::error(format!(
                                                "spread expression type mismatch: expected {:?}, found {:?}",
                                                t_expected, spread_ty
                                            ))
                                            .with_file(sf.path.clone()),
                                        );
                                    }
                                }

                                // This is struct initialization - validate fields
                                let has_spread = spread.is_some();
                                let mut provided_fields: std::collections::HashSet<String> =
                                    std::collections::HashSet::new();

                                for (key_expr, value_expr) in pairs {
                                    if let Expr::Ident(field_name) = key_expr {
                                        let field_name_str = &field_name.0;

                                        // Check for duplicate fields
                                        if provided_fields.contains(field_name_str) {
                                            reporter.emit(
                                                Diagnostic::error(format!(
                                                    "duplicate field '{}' in struct literal {}",
                                                    field_name_str, sname
                                                ))
                                                .with_file(sf.path.clone()),
                                            );
                                        } else {
                                            provided_fields.insert(field_name_str.clone());
                                        }

                                        if let Some((
                                            field_ty_np,
                                            _field_vis,
                                            _is_final,
                                            _default,
                                        )) = fields.get(field_name_str)
                                        {
                                            let value_ty =
                                                type_of_expr(value_expr, env, sf, reporter);
                                            let mut expected_ty = map_name_to_ty(field_ty_np);
                                            if let Ty::Named(ref mut pth) = expected_ty {
                                                qualify_named_with_imports(pth, env);
                                            }
                                            expected_ty = resolve_alias_ty(expected_ty, env);

                                            if !same_type(&expected_ty, &value_ty) {
                                                reporter.emit(
                                                    Diagnostic::error(format!(
                                                        "type mismatch for field '{}' in struct {}: expected {:?}, found {:?}",
                                                        field_name_str, sname, expected_ty, value_ty
                                                    ))
                                                    .with_file(sf.path.clone()),
                                                );
                                            }
                                        } else {
                                            reporter.emit(
                                                Diagnostic::error(format!(
                                                    "unknown field '{}' in struct {}",
                                                    field_name_str, sname
                                                ))
                                                .with_file(sf.path.clone()),
                                            );
                                        }
                                    } else {
                                        reporter.emit(
                                            Diagnostic::error(
                                                "struct field names must be identifiers",
                                            )
                                            .with_file(sf.path.clone()),
                                        );
                                    }
                                }

                                // Check for missing required fields (only if no spread)
                                if !has_spread {
                                    for (field_name, (_, _, _, default)) in fields.iter() {
                                        if default.is_none()
                                            && !provided_fields.contains(field_name)
                                        {
                                            reporter.emit(
                                                Diagnostic::error(format!(
                                                    "missing required field '{}' in struct literal {}",
                                                    field_name, sname
                                                ))
                                                .with_file(sf.path.clone()),
                                            );
                                        }
                                    }
                                }

                                // Check moves in struct literal
                                check_moves_in_expr(e, env, sf, reporter, current_pkg, rp);
                            } else {
                                // Not a known struct - fall back to regular type checking
                                let t_actual = type_of_expr(e, env, sf, reporter);
                                check_moves_in_expr(e, env, sf, reporter, current_pkg, rp);
                                if !same_type(&t_expected, &t_actual) {
                                    reporter.emit(
                                        Diagnostic::error(format!(
                                            "return type mismatch: expected {:?}, found {:?}",
                                            t_expected, t_actual
                                        ))
                                        .with_file(sf.path.clone()),
                                    );
                                }
                            }
                        } else {
                            // Expected type is not Named - fall back to regular type checking
                            let t_actual = type_of_expr(e, env, sf, reporter);
                            check_moves_in_expr(e, env, sf, reporter, current_pkg, rp);
                            if !same_type(&t_expected, &t_actual) {
                                reporter.emit(
                                    Diagnostic::error(format!(
                                        "return type mismatch: expected {:?}, found {:?}",
                                        t_expected, t_actual
                                    ))
                                    .with_file(sf.path.clone()),
                                );
                            }
                        }
                    } else {
                        // Not a MapLit - regular type checking
                        let t_actual = type_of_expr(e, env, sf, reporter);
                        check_moves_in_expr(e, env, sf, reporter, current_pkg, rp);
                        if !same_type(&t_expected, &t_actual) {
                            reporter.emit(
                                Diagnostic::error(format!(
                                    "return type mismatch: expected {:?}, found {:?}",
                                    t_expected, t_actual
                                ))
                                .with_file(sf.path.clone()),
                            );
                        }
                        // Escape analysis: mark returned value as escaping
                        if let Expr::Ident(id) = e {
                            env.lifetime_env.mark_escape_return(&id.0);
                        }
                    }
                }
            }
        }
        Stmt::Labeled { label, stmt } => {
            // Determine if this label applies to a loop or switch
            let is_loop = matches!(stmt.as_ref(), Stmt::While { .. } | Stmt::For { .. });
            let is_switch = matches!(stmt.as_ref(), Stmt::Switch { .. });

            // Only push label for loops and switches (the only valid label targets)
            if is_loop || is_switch {
                env.push_label(label.0.clone(), is_loop);
                typecheck_stmt(stmt, env, sf, reporter, current_pkg, rp);
                env.pop_label();
            } else {
                // Label on non-loop/switch statement - still typecheck but warn
                // (break/continue to this label will fail at use site)
                typecheck_stmt(stmt, env, sf, reporter, current_pkg, rp);
            }
        }
        Stmt::Expr(e) => {
            // Evaluate for type errors and move checks; result is discarded
            let _ = type_of_expr(e, env, sf, reporter);
            check_moves_in_expr(e, env, sf, reporter, current_pkg, rp);
        }
        Stmt::Block(b) => {
            env.push();
            typecheck_block(b, env, sf, reporter, current_pkg, rp);
            env.pop_with_lifetime_check(sf, reporter);
        }
        Stmt::Unsafe(b) => {
            // Unsafe block: enter unsafe context, typecheck contents, then exit
            env.push();
            env.unsafe_depth += 1;
            typecheck_block(b, env, sf, reporter, current_pkg, rp);
            env.unsafe_depth -= 1;
            env.pop_with_lifetime_check(sf, reporter);
        }
        Stmt::Throw(e) => {
            // Type-check the thrown expression
            let throw_ty = type_of_expr(e, env, sf, reporter);
            check_moves_in_expr(e, env, sf, reporter, current_pkg, rp);

            // Effect system: record throws effect
            env.effect_env.record_throws();

            // Check if we're inside a finally block - throw in finally replaces pending exception
            // The spec says: "finally may rethrow or replace the pending exception explicitly"
            // This is allowed but we emit a warning since it can be confusing
            // Note: We allow this since the spec explicitly allows "rethrow or replace"
            // but disallow return since that "moves values out"

            // Safety Rule 2: Check for exclusive borrows at throw point.
            // Exclusive borrows active at throw will cross to catch handlers.
            if let Some(excl) = env.excl_borrows.last() {
                for borrow_name in excl {
                    reporter.emit(
                        Diagnostic::warning(format!(
                            "throw while '{}' is exclusively borrowed; \
                            borrow will cross throw boundary to catch handler",
                            borrow_name
                        ))
                        .with_file(sf.path.clone()),
                    );
                }
            }

            // Verify the thrown type is a valid exception type
            // For now, we just ensure it's a named type (exception types are structs)
            if !matches!(throw_ty, Ty::Named(_) | Ty::Unknown) {
                reporter.emit(
                    Diagnostic::error(format!(
                        "throw expression must be an exception type, found '{}'",
                        ty_to_string(&throw_ty)
                    ))
                    .with_file(sf.path.clone()),
                );
            }
        }
        Stmt::Panic(e) => {
            // Type-check the panic message expression
            let panic_ty = type_of_expr(e, env, sf, reporter);
            check_moves_in_expr(e, env, sf, reporter, current_pkg, rp);

            // Verify the panic message is a String type
            if !matches!(panic_ty, Ty::String | Ty::Unknown) {
                reporter.emit(
                    Diagnostic::error(format!(
                        "panic message must be a String, found '{}'",
                        ty_to_string(&panic_ty)
                    ))
                    .with_file(sf.path.clone()),
                );
            }
        }
    }
}

fn typecheck_func(
    f: &AS::FuncDecl,
    current_pkg: &str,
    current_module: Option<&str>,
    conc: &std::sync::Arc<ConcTraits>,
    rp: &ResolvedProgram,
    sf: &SourceFile,
    reporter: &mut Reporter,
    free_fns: &std::sync::Arc<std::collections::HashMap<(String, String), Vec<AS::FuncSig>>>,
    module_fns: &std::sync::Arc<
        std::collections::HashMap<(String, String, String), Vec<AS::FuncSig>>,
    >,
    struct_fields: &std::sync::Arc<StructFieldsIndex>,
    imported_modules: &std::sync::Arc<std::collections::HashMap<String, String>>,
    star_import_pkgs: &std::sync::Arc<Vec<String>>,
    type_aliases: &std::sync::Arc<std::collections::HashMap<(String, String), AS::NamePath>>,
    enum_variants: &std::sync::Arc<
        std::collections::HashMap<(String, String, String), Vec<AS::NamePath>>,
    >,
    types_needing_drop: &std::sync::Arc<TypesNeedingDrop>,
    extern_funcs: &std::sync::Arc<ExternFuncsIndex>,
    unsafe_funcs: &std::sync::Arc<UnsafeFuncsIndex>,
    copy_types: &std::sync::Arc<CopyTypesIndex>,
    implements: &std::sync::Arc<ImplementsIndex>,
) -> Option<FunctionEscapeInfo> {
    if let Some(body) = &f.body {
        // Check if this is main() - allows await for implicit async entry point
        let is_main = f.sig.name.0 == "main";
        let mut env = Env::new(
            Some(current_pkg.to_string()),
            current_module.map(|s| s.to_string()),
            conc.clone(),
            f.sig.is_async,
            is_main,         // Allow await in main() for implicit async entry
            f.sig.is_unsafe, // Pass whether this function is unsafe
            free_fns.clone(),
            module_fns.clone(),
            struct_fields.clone(),
            imported_modules.clone(),
            star_import_pkgs.clone(),
            type_aliases.clone(),
            enum_variants.clone(),
            types_needing_drop.clone(),
            extern_funcs.clone(),
            unsafe_funcs.clone(),
            copy_types.clone(),
            implements.clone(),
        );

        // Set type parameters for generic functions
        if !f.sig.generics.is_empty() {
            env.set_type_params(&f.sig.generics);
        }

        // Set the expected return type for this function's body. For async functions,
        // we still check the inner type 'T' (external callers see Task<T>).
        let mut ret_ty: Ty = if let Some(ref np) = f.sig.ret {
            env.map_name_to_ty_with_params(np)
        } else {
            Ty::Void
        };
        if let Ty::Named(ref mut pth) = ret_ty {
            qualify_named_with_imports(pth, &env);
        }
        ret_ty = resolve_alias_ty(ret_ty, &env);
        env.expected_ret = ret_ty;
        // Params are considered initialized and use declare_param for lifetime tracking
        for p in &f.sig.params {
            let mut ty = env.map_name_to_ty_with_params(&p.ty);
            if let Ty::Named(ref mut pth) = ty {
                qualify_named_with_imports(pth, &env);
            }
            ty = resolve_alias_ty(ty, &env);
            let needs_drop = env.needs_drop(&ty);
            let drop_ty_name = env.drop_ty_name(&ty);
            let decl_order = env.next_decl_order();
            env.declare_param(
                &p.name.0,
                LocalInfo {
                    ty,
                    is_final: false,
                    initialized: true,
                    moved: false,
                    move_state: MoveState::Available,
                    num: numty_from_namepath(&p.ty),
                    col_kind: None,
                    needs_drop,
                    drop_ty_name,
                    decl_order,
                },
            );
        }

        // Async capture bounds: all parameters to async functions must be Sendable
        // because they are transferred across task boundaries when the task is spawned.
        // This is similar to Rust's `Send` bound on async function arguments.
        if f.sig.is_async {
            for p in &f.sig.params {
                if let Some(li) = env.get(&p.name.0) {
                    let ty = &li.ty;
                    // Skip primitive types - they are always Sendable
                    if matches!(
                        ty,
                        Ty::Int
                            | Ty::Float
                            | Ty::Bool
                            | Ty::Char
                            | Ty::String
                            | Ty::Bytes
                            | Ty::Void
                    ) {
                        continue;
                    }
                    // Check if the type is Sendable
                    if !is_sendable(&env, ty) {
                        let tname = match ty {
                            Ty::Named(path) => join_path(path),
                            Ty::Generic { path, .. } => join_path(path),
                            _ => format!("{:?}", ty),
                        };
                        reporter.emit(
                            Diagnostic::error(format!(
                                "async function parameter '{}' has type '{}' which is not Sendable; \
                                 async function parameters must be Sendable to cross task boundaries",
                                p.name.0, tname
                            ))
                            .with_file(sf.path.clone()),
                        );
                    }
                    // Also check that the parameter doesn't hold a borrow
                    // (borrowed references cannot be sent across task boundaries)
                    if let Some(borrow_info) = env.lifetime_env.get_local_borrow_info(&p.name.0) {
                        let origin_desc = match &borrow_info.origin {
                            lifetime::BorrowOrigin::Local(src) => format!("'{}'", src),
                            lifetime::BorrowOrigin::Param(src) => format!("parameter '{}'", src),
                            lifetime::BorrowOrigin::Field(obj, field_path) => {
                                format!("field '{}.{}'", obj, field_path.join("."))
                            }
                            lifetime::BorrowOrigin::Provider(prov) => {
                                format!("provider '{}'", prov)
                            }
                            lifetime::BorrowOrigin::Unknown => "unknown source".to_string(),
                        };
                        reporter.emit(
                            Diagnostic::error(format!(
                                "async function parameter '{}' holds a borrow of {}; \
                                 borrowed references cannot cross task boundaries",
                                p.name.0, origin_desc
                            ))
                            .with_file(sf.path.clone()),
                        );
                    }
                }
            }
        }

        typecheck_block(body, &mut env, sf, reporter, current_pkg, rp);
        // provider-tied borrow scaffolding: ensure no provider borrows escape function
        if let Some(root) = env.prov_borrows.first()
            && !root.is_empty()
        {
            reporter.emit(
                Diagnostic::error(
                    "provider-tied borrows (from borrowFromProvider) cannot escape function scope",
                )
                .with_file(sf.path.clone()),
            );
        }
        // Exclusive borrows must not escape the function body without a matching release.
        if let Some(root) = env.excl_borrows.first()
            && !root.is_empty()
        {
            reporter.emit(
                Diagnostic::error(
                    "exclusive borrows cannot escape function scope; missing release() for some borrows",
                )
                .with_file(sf.path.clone()),
            );
        }

        // Lifetime inference: check that no borrows escape function scope
        let lifetime_errors = env.lifetime_env.check_function_exit_full();
        for err in lifetime_errors {
            reporter.emit(Diagnostic::error(err.to_message()).with_file(sf.path.clone()));
        }

        // Move/drop validation for types that need dropping:
        // - For types with EXPLICIT deinit:
        //   - Partial moves are rejected (deinit might access moved fields)
        //   - Conditional moves use CondDrop (handled by drop flags)
        // - For types with droppable fields but NO explicit deinit:
        //   - Partial moves are allowed (we emit per-field drops)
        //   - Conditional moves are handled with CondDrop
        // - Values fully moved on all paths have drops suppressed
        for name in env.all_names() {
            if let Some(li) = env.get(&name) {
                if li.needs_drop {
                    let has_explicit_deinit = env.has_explicit_deinit(&li.ty);
                    match &li.move_state {
                        MoveState::Available => {
                            // Value is still owned in this scope; will be dropped normally.
                        }
                        MoveState::FullyMoved => {
                            // Fully moved out on all paths – drop is suppressed via escape info.
                        }
                        MoveState::ConditionallyMoved => {
                            // Conditional moves are now handled via CondDrop IR instruction.
                            // The lowering phase emits drop flags and conditional drops,
                            // ensuring deinit is called exactly once (or zero times if moved).
                        }
                        MoveState::PartiallyMoved(fields) => {
                            if !fields.is_empty() && has_explicit_deinit {
                                // Only reject partial moves for types with explicit deinit
                                reporter.emit(
                                    Diagnostic::error(format!(
                                        "partial move of fields {:?} from value '{}' of type '{}' is not allowed; \
                                         types with deinit functions must be moved or dropped as a whole",
                                        fields,
                                        name,
                                        ty_to_string(&li.ty)
                                    ))
                                    .with_file(sf.path.clone()),
                                );
                            }
                            // For types without explicit deinit, partial moves are allowed.
                            // The lowering phase will emit per-field drops for unmoved fields.
                        }
                    }
                }
            }
        }

        // Escape analysis finalization: set allocation strategies based on escape state
        // Non-escaping locals can use stack allocation for better performance
        env.lifetime_env.finalize_allocation_strategies();

        // Extract escape analysis results to pass to IR lowering
        return Some(extract_escape_info(&env));
    }
    None
}

/// Extract escape analysis information from the type checking environment
fn extract_escape_info(env: &Env) -> FunctionEscapeInfo {
    use lifetime::AllocStrategy as LifetimeAlloc;

    let mut func_info = FunctionEscapeInfo::new();

    // Get allocation info for all locals tracked in the lifetime environment
    for (name, local_lt) in env.lifetime_env.get_all_locals() {
        let alloc_strategy = match &local_lt.alloc_strategy {
            LifetimeAlloc::Stack => AllocStrategy::Stack,
            LifetimeAlloc::RefCounted => AllocStrategy::RefCounted,
            LifetimeAlloc::UniqueOwned => AllocStrategy::UniqueOwned,
            LifetimeAlloc::Region(id) => AllocStrategy::Region(id.0),
        };

        // Get drop info from the regular env if available
        let (mut needs_drop, mut drop_ty_name, move_state, ty_opt) = env
            .get(&name)
            .map(|li| {
                (
                    li.needs_drop,
                    li.drop_ty_name.clone(),
                    li.move_state.clone(),
                    Some(li.ty.clone()),
                )
            })
            .unwrap_or((false, None, MoveState::Available, None));

        // If the value has been fully moved out on all paths, it no longer needs a drop.
        if move_state.is_definitely_moved() {
            needs_drop = false;
            drop_ty_name = None;
        }

        // Convert typeck MoveState to escape_results MoveState
        let escape_move_state = match &move_state {
            MoveState::Available => escape_results::MoveState::Available,
            MoveState::FullyMoved => escape_results::MoveState::FullyMoved,
            MoveState::ConditionallyMoved => escape_results::MoveState::ConditionallyMoved,
            MoveState::PartiallyMoved(fields) => {
                escape_results::MoveState::PartiallyMoved(fields.clone())
            }
        };

        // Determine if type has explicit deinit and collect field drop info
        let (has_explicit_deinit, field_drop_info) = if let Some(ref ty) = ty_opt {
            extract_type_drop_info(env, ty)
        } else {
            (false, std::collections::HashMap::new())
        };

        func_info.add_local(
            &name,
            LocalEscapeInfo {
                alloc_strategy,
                needs_drop,
                drop_ty_name,
                move_state: escape_move_state,
                field_drop_info,
                has_explicit_deinit,
            },
        );
    }

    func_info
}

/// Extract drop information for a type: whether it has explicit deinit and per-field drop info
fn extract_type_drop_info(
    env: &Env,
    ty: &Ty,
) -> (
    bool,
    std::collections::HashMap<String, escape_results::FieldDropInfo>,
) {
    let mut field_drop_info = std::collections::HashMap::new();
    let has_explicit_deinit;

    match ty {
        Ty::Named(path) | Ty::Generic { path, .. } => {
            if path.is_empty() {
                return (false, field_drop_info);
            }
            let type_name = path.last().unwrap().clone();
            let pkg = if path.len() > 1 {
                path[..path.len() - 1].join(".")
            } else {
                env.current_pkg.clone().unwrap_or_default()
            };

            // Check if this type has an explicit deinit function (not synthetic)
            has_explicit_deinit = matches!(
                env.types_needing_drop
                    .get(&(pkg.clone(), type_name.clone())),
                Some(DeinitKind::Explicit(_))
            );

            // Get struct fields and their drop info
            if let Some(fields) = env.struct_fields.get(&(pkg.clone(), type_name)) {
                for (field_name, (field_ty, _, _, _)) in fields {
                    // Convert AS::NamePath to Ty for drop checking
                    let field_ty_as_ty = namepath_to_ty(field_ty);
                    let field_needs_drop = env.needs_drop(&field_ty_as_ty);
                    let field_drop_ty_name = if field_needs_drop {
                        env.drop_ty_name(&field_ty_as_ty)
                    } else {
                        None
                    };
                    field_drop_info.insert(
                        field_name.clone(),
                        escape_results::FieldDropInfo {
                            needs_drop: field_needs_drop,
                            drop_ty_name: field_drop_ty_name,
                        },
                    );
                }
            }
        }
        _ => {
            has_explicit_deinit = false;
        }
    }

    (has_explicit_deinit, field_drop_info)
}

fn typecheck_module(
    m: &AS::ModuleDecl,
    current_pkg: &str,
    conc: &std::sync::Arc<ConcTraits>,
    rp: &ResolvedProgram,
    sf: &SourceFile,
    reporter: &mut Reporter,
    free_fns: &std::sync::Arc<std::collections::HashMap<(String, String), Vec<AS::FuncSig>>>,
    module_fns: &std::sync::Arc<
        std::collections::HashMap<(String, String, String), Vec<AS::FuncSig>>,
    >,
    struct_fields: &std::sync::Arc<StructFieldsIndex>,
    imported_modules: &std::sync::Arc<std::collections::HashMap<String, String>>,
    star_import_pkgs: &std::sync::Arc<Vec<String>>,
    type_aliases: &std::sync::Arc<std::collections::HashMap<(String, String), AS::NamePath>>,
    enum_variants: &std::sync::Arc<
        std::collections::HashMap<(String, String, String), Vec<AS::NamePath>>,
    >,
    types_needing_drop: &std::sync::Arc<TypesNeedingDrop>,
    extern_funcs: &std::sync::Arc<ExternFuncsIndex>,
    unsafe_funcs: &std::sync::Arc<UnsafeFuncsIndex>,
    copy_types: &std::sync::Arc<CopyTypesIndex>,
    implements: &std::sync::Arc<ImplementsIndex>,
    escape_results: &mut EscapeAnalysisResults,
) {
    for f in &m.items {
        if let Some(func_escape_info) = typecheck_func(
            f,
            current_pkg,
            Some(&m.name.0),
            conc,
            rp,
            sf,
            reporter,
            free_fns,
            module_fns,
            struct_fields,
            imported_modules,
            star_import_pkgs,
            type_aliases,
            enum_variants,
            types_needing_drop,
            extern_funcs,
            unsafe_funcs,
            copy_types,
            implements,
        ) {
            // Store escape info with function key: (pkg, module, func_name)
            let key = (
                current_pkg.to_string(),
                Some(m.name.0.clone()),
                f.sig.name.0.clone(),
            );
            escape_results.add_function(key, func_escape_info);
        }
    }
}

fn pkg_string(ast: &AS::FileAst) -> Option<String> {
    ast.package.as_ref().map(|p| p.to_string())
}

fn validate_generic_bounds(
    current_pkg: &str,
    ast: &AS::FileAst,
    rp: &ResolvedProgram,
    reporter: &mut Reporter,
    sf: &SourceFile,
) {
    // Helper to check one bound path refers to an interface symbol.
    let mut check_bound = |np: &AS::NamePath| {
        let parts: Vec<String> = np.path.iter().map(|i| i.0.clone()).collect();
        match lookup_symbol_kind(rp, current_pkg, &parts) {
            Some(ResolvedKind::Interface) => {}
            Some(_) => reporter.emit(
                Diagnostic::error(format!(
                    "generic bound '{}' must name an interface",
                    parts.join(".")
                ))
                .with_file(sf.path.clone()),
            ),
            None => reporter.emit(
                Diagnostic::error(format!("generic bound '{}' not found", parts.join(".")))
                    .with_file(sf.path.clone()),
            ),
        }
    };

    for d in &ast.decls {
        match d {
            AS::Decl::Function(f) => {
                for g in &f.sig.generics {
                    if let Some(b) = &g.bound {
                        check_bound(b);
                    }
                }
            }
            AS::Decl::Struct(s) => {
                for g in &s.generics {
                    if let Some(b) = &g.bound {
                        check_bound(b);
                    }
                }
            }
            AS::Decl::Enum(e) => {
                for g in &e.generics {
                    if let Some(b) = &g.bound {
                        check_bound(b);
                    }
                }
            }
            AS::Decl::Module(m) => {
                for func in &m.items {
                    for g in &func.sig.generics {
                        if let Some(b) = &g.bound {
                            check_bound(b);
                        }
                    }
                }
            }
            AS::Decl::Interface(i) => {
                for g in &i.generics {
                    if let Some(b) = &g.bound {
                        check_bound(b);
                    }
                }
            }
            _ => {}
        }
    }
}

pub fn typecheck_project(
    _root: &std::path::Path,
    files: &[(SourceFile, AS::FileAst)],
    rp: &ResolvedProgram,
    reporter: &mut Reporter,
) -> EscapeAnalysisResults {
    // Initialize escape analysis results collector
    let mut escape_results = EscapeAnalysisResults::new();

    // Collect type aliases across files: (pkg, alias) -> NamePath
    fn build_type_aliases(
        files: &[(SourceFile, AS::FileAst)],
    ) -> std::collections::HashMap<(String, String), AS::NamePath> {
        let mut aliases: std::collections::HashMap<(String, String), AS::NamePath> =
            std::collections::HashMap::new();
        for (_sf, ast) in files {
            let pkg = match pkg_string(ast) {
                Some(p) => p,
                None => continue,
            };
            for d in &ast.decls {
                if let AS::Decl::TypeAlias(ta) = d {
                    aliases.insert((pkg.clone(), ta.name.0.clone()), ta.aliased.clone());
                }
            }
        }
        aliases
    }
    // Build struct fields index for member typing (includes both structs and providers)
    fn build_struct_fields(files: &[(SourceFile, AS::FileAst)]) -> StructFieldsIndex {
        let mut idx: StructFieldsIndex = std::collections::HashMap::new();
        for (_sf, ast) in files {
            let pkg = match pkg_string(ast) {
                Some(p) => p,
                None => continue,
            };
            for d in &ast.decls {
                // Handle struct fields
                if let AS::Decl::Struct(s) = d {
                    let mut fmap: std::collections::HashMap<String, FieldMetadata> =
                        std::collections::HashMap::new();
                    for f in &s.fields {
                        // Extract @default attribute if present
                        let default_expr = f.attrs.iter().find_map(|attr| {
                            let attr_name = attr
                                .name
                                .path
                                .iter()
                                .map(|i| i.0.as_str())
                                .collect::<Vec<_>>()
                                .join(".");
                            if attr_name == "default" {
                                let args = parse_attr_args(attr.args.as_deref());
                                parse_default_expr(&args)
                            } else {
                                None
                            }
                        });
                        // Store (type, visibility, is_final, default_expr) for each field
                        fmap.insert(
                            f.name.0.clone(),
                            (f.ty.clone(), f.vis.clone(), f.is_final, default_expr),
                        );
                    }
                    idx.insert((pkg.clone(), s.name.0.clone()), fmap);
                }
                // Handle provider fields (providers have fields too)
                if let AS::Decl::Provider(p) = d {
                    let mut fmap: std::collections::HashMap<String, FieldMetadata> =
                        std::collections::HashMap::new();
                    for f in &p.fields {
                        // Providers don't have @default attributes typically, but check anyway
                        let default_expr = f.attrs.iter().find_map(|attr| {
                            let attr_name = attr
                                .name
                                .path
                                .iter()
                                .map(|i| i.0.as_str())
                                .collect::<Vec<_>>()
                                .join(".");
                            if attr_name == "default" {
                                let args = parse_attr_args(attr.args.as_deref());
                                parse_default_expr(&args)
                            } else {
                                None
                            }
                        });
                        // Store (type, visibility, is_final, default_expr) for each field
                        fmap.insert(
                            f.name.0.clone(),
                            (f.ty.clone(), f.vis.clone(), f.is_final, default_expr),
                        );
                    }
                    idx.insert((pkg.clone(), p.name.0.clone()), fmap);
                }
            }
        }
        idx
    }

    // 0) Pre-scan modules for deinit lookup and collect interfaces
    let mut module_index: std::collections::HashMap<(String, String), Vec<AS::FuncSig>> =
        std::collections::HashMap::new();
    let mut ifc_index: std::collections::HashMap<
        (String, String),
        (Vec<String>, Vec<AS::InterfaceMethod>, Vec<AS::NamePath>),
    > = std::collections::HashMap::new();
    for (_sf, ast) in files {
        let pkg = match pkg_string(ast) {
            Some(p) => p,
            None => continue,
        };
        for d in &ast.decls {
            if let AS::Decl::Module(m) = d {
                let key = (pkg.clone(), m.name.0.clone());
                let mut sigs: Vec<AS::FuncSig> = Vec::new();
                for f in &m.items {
                    sigs.push(f.sig.clone());
                }
                module_index.insert(key, sigs);
            }
            if let AS::Decl::Interface(i) = d {
                let key = (pkg.clone(), i.name.0.clone());
                // Capture generic parameter names for substitution during conformance checks
                let gen_names: Vec<String> = i.generics.iter().map(|g| g.name.0.clone()).collect();
                ifc_index.insert(key, (gen_names, i.methods.clone(), i.extends.clone()));
            }
        }
    }

    // Build function indices for return type lookup (free functions and module functions)
    let mut free_fn_index: std::collections::HashMap<(String, String), Vec<AS::FuncSig>> =
        std::collections::HashMap::new();
    let mut module_fn_index: std::collections::HashMap<(String, String, String), Vec<AS::FuncSig>> =
        std::collections::HashMap::new();
    for (_sf, ast) in files {
        let pkg = match pkg_string(ast) {
            Some(p) => p,
            None => continue,
        };
        for d in &ast.decls {
            match d {
                Decl::Function(f) => {
                    free_fn_index
                        .entry((pkg.clone(), f.sig.name.0.clone()))
                        .or_default()
                        .push(f.sig.clone());
                }
                Decl::Module(m) => {
                    for f in &m.items {
                        module_fn_index
                            .entry((pkg.clone(), m.name.0.clone(), f.sig.name.0.clone()))
                            .or_default()
                            .push(f.sig.clone());
                    }
                }
                _ => {}
            }
        }
    }
    // Seed stdlib function signatures from .arth files to aid return-type lookup
    // Stdlib is loaded from stdlib/src/*.arth files which are the single source of truth.
    let stdlib_path = std::path::Path::new("stdlib/src");
    if stdlib_path.exists() {
        if let Ok(stdlib) = StdlibIndex::load(stdlib_path) {
            seed_stdlib_from_index(&stdlib, &mut module_fn_index);
        }
    }
    let free_fn_index = std::sync::Arc::new(free_fn_index);
    let module_fn_index = std::sync::Arc::new(module_fn_index);

    // Reject free functions at typecheck time as well
    for (sf, ast) in files {
        for d in &ast.decls {
            if let Decl::Function(f) = d {
                reporter.emit(
                    Diagnostic::error("free functions are not supported; declare inside a module")
                        .with_file(sf.path.clone())
                        .with_span(f.span.clone()),
                );
            }
        }
    }

    // 1) Validate generic bounds, struct fields, and interface extends/method forms
    for (sf, ast) in files {
        if let Some(pkg) = pkg_string(ast) {
            validate_generic_bounds(&pkg, ast, rp, reporter, sf);
            validate_structs(&pkg, ast, rp, reporter, sf, &module_index);
            validate_interfaces(&pkg, ast, rp, reporter, sf);
            validate_enums(&pkg, ast, rp, reporter, sf);
            validate_exceptions(&pkg, ast, rp, reporter, sf, &module_index);
            validate_ffi_declarations(&pkg, ast, reporter, sf);
        }
    }

    // 2) Interface satisfaction via module implements
    for (sf, ast) in files {
        if let Some(pkg) = pkg_string(ast) {
            check_module_implements(&pkg, ast, rp, reporter, sf, &ifc_index);
        }
    }

    // 2.5) Derive concurrency traits
    let conc_traits = std::sync::Arc::new(derive_concurrency_traits(files, rp));
    // 2.6) Struct fields map for member typing
    let struct_fields = std::sync::Arc::new(build_struct_fields(files));

    let implements = std::sync::Arc::new(build_implements_index(files));

    let type_aliases = std::sync::Arc::new(build_type_aliases(files));

    // Validate type aliases: detect cycles and report errors
    fn validate_type_aliases(
        files: &[(SourceFile, AS::FileAst)],
        aliases: &std::collections::HashMap<(String, String), AS::NamePath>,
        reporter: &mut Reporter,
    ) {
        use std::collections::HashSet;

        // Helper to check if a given alias leads to a cycle
        fn detect_cycle(
            pkg: &str,
            name: &str,
            aliases: &std::collections::HashMap<(String, String), AS::NamePath>,
            visited: &mut HashSet<(String, String)>,
            path: &mut Vec<(String, String)>,
        ) -> Option<Vec<(String, String)>> {
            let key = (pkg.to_string(), name.to_string());
            if path.contains(&key) {
                // Found a cycle - return the cycle path
                let cycle_start = path.iter().position(|k| k == &key).unwrap();
                return Some(path[cycle_start..].to_vec());
            }
            if visited.contains(&key) {
                return None; // Already checked, no cycle from here
            }
            visited.insert(key.clone());
            path.push(key.clone());

            if let Some(target) = aliases.get(&(pkg.to_string(), name.to_string())) {
                // Get the target type name (NamePath is Vec<Ident>, Ident is String)
                let parts: Vec<String> = target.path.iter().map(|id| id.0.clone()).collect();
                let (target_pkg, target_name) = if parts.len() == 1 {
                    (pkg.to_string(), parts[0].clone())
                } else {
                    (
                        parts[..parts.len() - 1].join("."),
                        parts.last().unwrap().clone(),
                    )
                };
                // Only follow if target is also an alias
                if aliases.contains_key(&(target_pkg.clone(), target_name.clone())) {
                    let result = detect_cycle(&target_pkg, &target_name, aliases, visited, path);
                    path.pop();
                    return result;
                }
            }
            path.pop();
            None
        }

        let mut visited = HashSet::new();
        let mut reported_cycles = HashSet::new();

        for (sf, ast) in files {
            let Some(pkg) = pkg_string(ast) else {
                continue;
            };
            for d in &ast.decls {
                if let AS::Decl::TypeAlias(ta) = d {
                    let mut path = Vec::new();
                    if let Some(cycle) =
                        detect_cycle(&pkg, &ta.name.0, aliases, &mut visited, &mut path)
                    {
                        // Only report each cycle once (by the first alias in the cycle)
                        let cycle_key: Vec<_> =
                            cycle.iter().map(|(p, n)| format!("{}.{}", p, n)).collect();
                        let cycle_sig = cycle_key.join(" -> ");
                        if !reported_cycles.contains(&cycle_sig) {
                            reported_cycles.insert(cycle_sig);
                            let cycle_str = cycle
                                .iter()
                                .map(|(p, n)| {
                                    if p.is_empty() {
                                        n.clone()
                                    } else {
                                        format!("{}.{}", p, n)
                                    }
                                })
                                .collect::<Vec<_>>()
                                .join(" -> ");
                            reporter.emit(
                                Diagnostic::error(format!(
                                    "circular type alias: {} -> {}",
                                    cycle_str,
                                    if cycle.is_empty() {
                                        ta.name.0.clone()
                                    } else {
                                        let (p, n) = &cycle[0];
                                        if p.is_empty() {
                                            n.clone()
                                        } else {
                                            format!("{}.{}", p, n)
                                        }
                                    }
                                ))
                                .with_span(ta.span)
                                .with_file(sf.path.clone()),
                            );
                        }
                    }
                }
            }
        }
    }
    validate_type_aliases(files, &type_aliases, reporter);

    // Build enum variant index: (pkg, enum, variant) -> payload types
    fn build_enum_variants(
        files: &[(SourceFile, AS::FileAst)],
    ) -> std::collections::HashMap<(String, String, String), Vec<AS::NamePath>> {
        let mut m = std::collections::HashMap::new();
        for (_sf, ast) in files {
            let Some(pkg) = pkg_string(ast) else {
                continue;
            };
            for d in &ast.decls {
                if let AS::Decl::Enum(e) = d {
                    for v in &e.variants {
                        match v {
                            AS::EnumVariant::Unit { name, .. } => {
                                m.insert(
                                    (pkg.clone(), e.name.0.clone(), name.0.clone()),
                                    Vec::new(),
                                );
                            }
                            AS::EnumVariant::Tuple { name, types, .. } => {
                                m.insert(
                                    (pkg.clone(), e.name.0.clone(), name.0.clone()),
                                    types.clone(),
                                );
                            }
                        }
                    }
                }
            }
        }
        m
    }
    let enum_variants = std::sync::Arc::new(build_enum_variants(files));

    // Build types_needing_drop: types that have explicit or synthetic deinit
    fn build_types_needing_drop(
        module_index: &std::collections::HashMap<(String, String), Vec<AS::FuncSig>>,
        files: &[(SourceFile, AS::FileAst)],
        struct_fields: &StructFieldsIndex,
    ) -> TypesNeedingDrop {
        let mut result = TypesNeedingDrop::new();

        // Phase 1: Collect explicit deinit types (structs and providers)
        for (_sf, ast) in files {
            let Some(pkg) = pkg_string(ast) else {
                continue;
            };
            for d in &ast.decls {
                // Check for struct deinit
                if let AS::Decl::Struct(s) = d {
                    let struct_name = &s.name.0;
                    let module_name = format!("{}Fns", struct_name);
                    let key = (pkg.clone(), module_name.clone());
                    // Check if the companion module has a deinit function with the struct as first param
                    if let Some(sigs) = module_index.get(&key) {
                        let has_deinit = sigs.iter().any(|sig| {
                            sig.name.0 == "deinit"
                                && sig
                                    .params
                                    .first()
                                    .and_then(|p| p.ty.path.last().map(|i| &i.0))
                                    == Some(struct_name)
                        });
                        if has_deinit {
                            result.insert(
                                (pkg.clone(), struct_name.clone()),
                                DeinitKind::Explicit(module_name),
                            );
                        }
                    }
                }
                // Check for provider deinit
                if let AS::Decl::Provider(p) = d {
                    let provider_name = &p.name.0;
                    let module_name = format!("{}Fns", provider_name);
                    let key = (pkg.clone(), module_name.clone());
                    // Check if the companion module has a deinit function with the provider as first param
                    if let Some(sigs) = module_index.get(&key) {
                        let has_deinit = sigs.iter().any(|sig| {
                            sig.name.0 == "deinit"
                                && sig
                                    .params
                                    .first()
                                    .and_then(|p| p.ty.path.last().map(|i| &i.0))
                                    == Some(provider_name)
                        });
                        if has_deinit {
                            result.insert(
                                (pkg.clone(), provider_name.clone()),
                                DeinitKind::Explicit(module_name),
                            );
                        }
                    }
                }
            }
        }

        // Phase 2: Iteratively add types with droppable fields (synthetic deinit)
        // This is a fixpoint computation since types can contain other types
        loop {
            let mut changed = false;
            for (_sf, ast) in files {
                let Some(pkg) = pkg_string(ast) else {
                    continue;
                };
                for d in &ast.decls {
                    if let AS::Decl::Struct(s) = d {
                        let struct_name = &s.name.0;
                        let key = (pkg.clone(), struct_name.clone());

                        // Skip if already registered (explicit or synthetic)
                        if result.contains_key(&key) {
                            continue;
                        }

                        // Check if any field needs dropping
                        let mut fields_needing_drop: Vec<(String, String)> = Vec::new();
                        if let Some(field_map) =
                            struct_fields.get(&(pkg.clone(), struct_name.clone()))
                        {
                            // Process fields in reverse order for proper drop ordering
                            let mut field_list: Vec<_> = field_map.iter().collect();
                            field_list.reverse();

                            for (field_name, (field_ty, _, _, _)) in field_list {
                                // Get the type key for the field
                                let field_type_key = if field_ty.path.len() > 1 {
                                    // Qualified type: pkg.TypeName
                                    let field_pkg = field_ty.path[..field_ty.path.len() - 1]
                                        .iter()
                                        .map(|i| i.0.clone())
                                        .collect::<Vec<_>>()
                                        .join(".");
                                    let field_type_name = field_ty
                                        .path
                                        .last()
                                        .map(|i| i.0.clone())
                                        .unwrap_or_default();
                                    (field_pkg, field_type_name)
                                } else {
                                    // Unqualified type: look up in same package
                                    let field_type_name = field_ty
                                        .path
                                        .first()
                                        .map(|i| i.0.clone())
                                        .unwrap_or_default();
                                    (pkg.clone(), field_type_name)
                                };

                                // Check if field type needs drop
                                if result.contains_key(&field_type_key) {
                                    let drop_ty_name =
                                        format!("{}.{}", field_type_key.0, field_type_key.1);
                                    fields_needing_drop.push((field_name.clone(), drop_ty_name));
                                }
                            }
                        }

                        // If any fields need dropping, create synthetic deinit
                        if !fields_needing_drop.is_empty() {
                            result.insert(key, DeinitKind::Synthetic(fields_needing_drop));
                            changed = true;
                        }
                    }
                }
            }
            if !changed {
                break;
            }
        }

        result
    }
    let types_needing_drop = std::sync::Arc::new(build_types_needing_drop(
        &module_index,
        files,
        &struct_fields,
    ));

    // Build extern functions index: (pkg, name) -> extern function info
    fn build_extern_funcs(
        files: &[(SourceFile, AS::FileAst)],
        reporter: &mut Reporter,
    ) -> ExternFuncsIndex {
        let mut result = ExternFuncsIndex::new();
        for (sf, ast) in files {
            let Some(pkg) = pkg_string(ast) else {
                continue;
            };
            for d in &ast.decls {
                if let AS::Decl::ExternFunc(ef) = d {
                    let param_types: Vec<AS::NamePath> =
                        ef.params.iter().map(|p| p.ty.clone()).collect();

                    // Parse FFI ownership attribute
                    let ownership = parse_ffi_ownership_attrs(&ef.attrs, sf, reporter);

                    let info = ExternFuncInfo {
                        param_types,
                        ret_type: ef.ret.clone(),
                        ownership,
                    };
                    result.insert((pkg.clone(), ef.name.0.clone()), info);
                }
            }
        }
        result
    }
    let extern_funcs = std::sync::Arc::new(build_extern_funcs(files, reporter));

    // Build unsafe functions index: (pkg, module_opt, name) for unsafe function declarations
    fn build_unsafe_funcs(files: &[(SourceFile, AS::FileAst)]) -> UnsafeFuncsIndex {
        let mut result = UnsafeFuncsIndex::new();
        for (_sf, ast) in files {
            let Some(pkg) = pkg_string(ast) else {
                continue;
            };
            for d in &ast.decls {
                match d {
                    AS::Decl::Function(f) if f.sig.is_unsafe => {
                        result.insert((pkg.clone(), None, f.sig.name.0.clone()));
                    }
                    AS::Decl::Module(m) => {
                        for f in &m.items {
                            if f.sig.is_unsafe {
                                result.insert((
                                    pkg.clone(),
                                    Some(m.name.0.clone()),
                                    f.sig.name.0.clone(),
                                ));
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        result
    }
    let unsafe_funcs = std::sync::Arc::new(build_unsafe_funcs(files));

    // Infer target type from module method signatures.
    // Looks at the first parameter of the first method to determine the target type.
    fn infer_target_type_from_methods(items: &[AS::FuncDecl]) -> Option<String> {
        for func in items {
            if let Some(first_param) = func.sig.params.first() {
                // Get the type name (last segment of the path)
                return first_param.ty.path.last().map(|i| i.0.clone());
            }
        }
        None
    }

    // Infer target type from module name using naming convention.
    // E.g., "PointFns" -> "Point", "StringOps" -> "String"
    fn infer_target_type_from_module_name(module_name: &str) -> Option<String> {
        for suffix in &["Fns", "Ops", "Impl", "Module"] {
            if module_name.ends_with(suffix) && module_name.len() > suffix.len() {
                return Some(module_name[..module_name.len() - suffix.len()].to_string());
            }
        }
        None
    }

    // Build copy types index: types that explicitly implement Copy via module declaration
    // or can be auto-derived because all fields are Copy types
    // Types with deinit are excluded from auto-derivation - they cannot be Copy.
    fn build_copy_types(
        files: &[(SourceFile, AS::FileAst)],
        struct_fields_idx: &StructFieldsIndex,
        types_needing_drop: &TypesNeedingDrop,
    ) -> CopyTypesIndex {
        let mut result = CopyTypesIndex::new();

        // Phase 1: Collect explicit Copy implementations from module declarations
        // e.g., `module PointFns implements Copy { ... }`
        // Target type is inferred from method signatures or module name convention
        for (_sf, ast) in files {
            let Some(pkg) = pkg_string(ast) else {
                continue;
            };
            for d in &ast.decls {
                if let AS::Decl::Module(m) = d {
                    // Check if this module implements Copy
                    let implements_copy = m
                        .implements
                        .iter()
                        .any(|ifc| ifc.path.last().map(|i| i.0.as_str()) == Some("Copy"));
                    if implements_copy {
                        // Try to infer target type:
                        // 1. First from method signatures (first parameter type)
                        // 2. Fall back to module name convention (e.g., PointFns -> Point)
                        let target_name = infer_target_type_from_methods(&m.items)
                            .or_else(|| infer_target_type_from_module_name(&m.name.0));
                        if let Some(name) = target_name {
                            if !name.is_empty() {
                                result.insert((pkg.clone(), name), CopyKind::Explicit);
                            }
                        }
                    }
                }
            }
        }

        // Phase 1b: Register all provider types as auto-Copy
        // Providers are handles to shared state and can be safely copied.
        // This enables the provider injection pattern where providers are passed
        // to functions without moving ownership.
        for (_sf, ast) in files {
            let Some(pkg) = pkg_string(ast) else {
                continue;
            };
            for d in &ast.decls {
                if let AS::Decl::Provider(p) = d {
                    let key = (pkg.clone(), p.name.0.clone());
                    result.insert(key, CopyKind::Derived);
                }
            }
        }

        // Phase 2: Auto-derive Copy for structs where all fields are Copy types
        // BUT exclude types that have a deinit function - they cannot be Copy.
        // Keep iterating until no new Copy types are discovered (fixed-point iteration)
        let mut changed = true;
        while changed {
            changed = false;
            for (_sf, ast) in files {
                let Some(pkg) = pkg_string(ast) else {
                    continue;
                };
                for d in &ast.decls {
                    if let AS::Decl::Struct(s) = d {
                        let key = (pkg.clone(), s.name.0.clone());
                        // Skip if already marked as Copy
                        if result.contains_key(&key) {
                            continue;
                        }
                        // Types with deinit cannot be Copy - they have destructors
                        if types_needing_drop.contains_key(&key) {
                            continue;
                        }
                        // Check if all fields are Copy types
                        let all_fields_copy = s
                            .fields
                            .iter()
                            .all(|f| is_field_type_copy(&f.ty, &pkg, &result, struct_fields_idx));
                        if all_fields_copy {
                            result.insert(key, CopyKind::Derived);
                            changed = true;
                        }
                    }
                }
            }
        }

        result
    }

    // Helper to check if a field type is Copy
    fn is_field_type_copy(
        ty: &AS::NamePath,
        current_pkg: &str,
        copy_types: &CopyTypesIndex,
        _struct_fields: &StructFieldsIndex,
    ) -> bool {
        let base_ty = map_name_to_ty(ty);
        match base_ty {
            // Primitives are always Copy
            Ty::Int | Ty::Float | Ty::Bool | Ty::Char | Ty::String | Ty::Bytes | Ty::Void => true,
            // Named types: check if they're in the copy_types index
            Ty::Named(path) => {
                if path.is_empty() {
                    return false;
                }
                let type_name = path.last().unwrap().clone();
                // Handle Owned<T> - never copy
                if type_name == "Owned" {
                    return false;
                }
                // Determine package
                let pkg = if path.len() > 1 {
                    path[..path.len() - 1].join(".")
                } else {
                    current_pkg.to_string()
                };
                copy_types.contains_key(&(pkg, type_name))
            }
            // Generic types like Task<T>, List<T> are not Copy by default
            Ty::Generic { path, .. } => {
                if path.is_empty() {
                    return false;
                }
                let type_name = path.last().unwrap().clone();
                // Owned<T> is never Copy
                if type_name == "Owned" {
                    return false;
                }
                // For generic types, check if the base type is Copy
                let pkg = if path.len() > 1 {
                    path[..path.len() - 1].join(".")
                } else {
                    current_pkg.to_string()
                };
                copy_types.contains_key(&(pkg, type_name))
            }
            // Tuple types: Copy if all elements are Copy (would need recursive check)
            // For now, conservatively return false
            Ty::Tuple(_) => false,
            // Function types are not Copy
            Ty::Function(_, _) => false,
            // Reference types are not Copy - they represent borrows with lifetime constraints
            Ty::Ref { .. } => false,
            // Never type is trivially Copy (code never reaches there)
            Ty::Never => true,
            // Unknown types are not Copy
            Ty::Unknown => false,
        }
    }

    let copy_types =
        std::sync::Arc::new(build_copy_types(files, &struct_fields, &types_needing_drop));

    // 3) Local expression typing and assignment rules
    for (sf, ast) in files {
        // Build import context for this file: module name -> package and star-imported package list
        let (imported_modules_map, star_imports_vec) = {
            let mut module_map: std::collections::HashMap<String, String> =
                std::collections::HashMap::new();
            let mut star_pkgs: Vec<String> = Vec::new();
            // Named imports: import pkg.Module; -> maps 'Module' -> 'pkg'
            for imp in &ast.imports {
                // star imports: remember the package for fallback resolution
                if imp.star {
                    if !imp.path.is_empty() {
                        let p: String = imp
                            .path
                            .iter()
                            .map(|i| i.0.clone())
                            .collect::<Vec<_>>()
                            .join(".");
                        star_pkgs.push(p);
                    }
                } else if imp.path.len() >= 2 {
                    let name = imp.path.last().unwrap().0.clone();
                    let pkg = imp.path[..imp.path.len() - 1]
                        .iter()
                        .map(|i| i.0.clone())
                        .collect::<Vec<_>>()
                        .join(".");
                    module_map.entry(name).or_insert(pkg);
                }
            }
            (
                std::sync::Arc::new(module_map),
                std::sync::Arc::new(star_pkgs),
            )
        };
        for d in &ast.decls {
            match d {
                Decl::Function(f) => {
                    if let Some(pkg) = pkg_string(ast) {
                        if let Some(func_escape_info) = typecheck_func(
                            f,
                            &pkg,
                            None,
                            &conc_traits,
                            rp,
                            sf,
                            reporter,
                            &free_fn_index,
                            &module_fn_index,
                            &struct_fields,
                            &imported_modules_map,
                            &star_imports_vec,
                            &type_aliases,
                            &enum_variants,
                            &types_needing_drop,
                            &extern_funcs,
                            &unsafe_funcs,
                            &copy_types,
                            &implements,
                        ) {
                            // Store escape info for free function: (pkg, None, func_name)
                            let key = (pkg.clone(), None, f.sig.name.0.clone());
                            escape_results.add_function(key, func_escape_info);
                        }
                    }
                }
                Decl::Module(m) => {
                    if let Some(pkg) = pkg_string(ast) {
                        typecheck_module(
                            m,
                            &pkg,
                            &conc_traits,
                            rp,
                            sf,
                            reporter,
                            &free_fn_index,
                            &module_fn_index,
                            &struct_fields,
                            &imported_modules_map,
                            &star_imports_vec,
                            &type_aliases,
                            &enum_variants,
                            &types_needing_drop,
                            &extern_funcs,
                            &unsafe_funcs,
                            &copy_types,
                            &implements,
                            &mut escape_results,
                        )
                    }
                }
                _ => {}
            }
        }
    }
    escape_results
}

// tests live in tests.rs

fn validate_structs(
    current_pkg: &str,
    ast: &AS::FileAst,
    rp: &ResolvedProgram,
    reporter: &mut Reporter,
    sf: &SourceFile,
    module_index: &std::collections::HashMap<(String, String), Vec<AS::FuncSig>>,
) {
    for d in &ast.decls {
        let AS::Decl::Struct(s) = d else { continue };
        // Collect this struct's generic parameter names to allow them as field types.
        let generic_names: std::collections::HashSet<String> =
            s.generics.iter().map(|g| g.name.0.clone()).collect();
        // Enforce: 'shared' modifier is not allowed on struct fields (provider-only)
        for f in &s.fields {
            if f.is_shared {
                reporter.emit(
                    Diagnostic::error(format!(
                        "'shared' is only allowed on provider fields and local variables; field '{}' in struct '{}' is invalid. For shared mutation, use Atomic<T>, an actor, or capability-guarded APIs.",
                        f.name.0, s.name.0
                    ))
                    .with_file(sf.path.clone()),
                );
            }
        }
        // Duplicate field names
        let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for fld in &s.fields {
            if !seen.insert(&fld.name.0) {
                reporter.emit(
                    Diagnostic::error(format!(
                        "duplicate field '{}' in struct {}",
                        fld.name.0, s.name.0
                    ))
                    .with_file(sf.path.clone()),
                );
            }
            // Field type must be known (primitive or declared symbol)
            let base_ty = map_name_to_ty(&fld.ty);
            let is_prim = !matches!(base_ty, Ty::Named(_) | Ty::Unknown);
            if !is_prim {
                let parts: Vec<String> = fld.ty.path.iter().map(|i| i.0.clone()).collect();
                // Permit referring to this struct's generic parameters directly (e.g., field of type `T`).
                if parts.len() == 1 && generic_names.contains(&parts[0]) {
                    continue;
                }
                if lookup_symbol_kind(rp, current_pkg, &parts).is_none() {
                    reporter.emit(
                        Diagnostic::error(format!(
                            "unknown field type '{}' in struct {}",
                            parts.join("."),
                            s.name.0
                        ))
                        .with_file(sf.path.clone()),
                    );
                }
            }
        }

        // Deinit constraint (soft): if struct has Shared/Atomic fields, recommend having a `deinit` function in `<StructName>Fns` module.
        let needs_deinit = s.fields.iter().any(|f| {
            let b = f.ty.path.last().map(|i| i.0.as_str()).unwrap_or("");
            b == "Shared" || b == "Atomic"
        });
        if needs_deinit {
            let mname = format!("{}Fns", s.name.0);
            let key = (current_pkg.to_string(), mname.clone());
            let ok = module_index.get(&key).is_some_and(|sigs| {
                sigs.iter().any(|sig| {
                    sig.name.0 == "deinit"
                        && (sig
                            .params
                            .first()
                            .and_then(|p| p.ty.path.last().map(|i| &i.0))
                            == Some(&s.name.0))
                })
            });
            if !ok {
                reporter.emit(Diagnostic::warning(format!("struct '{}' contains Shared/Atomic fields; consider providing {}.deinit({})", s.name.0, mname, s.name.0)).with_file(sf.path.clone()));
            }
        }

        // Deinit must-not-throw constraint: if deinit function exists, verify it doesn't have throws clause
        let mname = format!("{}Fns", s.name.0);
        let key = (current_pkg.to_string(), mname.clone());
        if let Some(sigs) = module_index.get(&key) {
            for sig in sigs {
                if sig.name.0 == "deinit"
                    && sig
                        .params
                        .first()
                        .and_then(|p| p.ty.path.last().map(|i| &i.0))
                        == Some(&s.name.0)
                {
                    // Check that deinit doesn't throw
                    if !sig.throws.is_empty() {
                        let throws_str: Vec<String> = sig
                            .throws
                            .iter()
                            .map(|np| {
                                np.path
                                    .iter()
                                    .map(|i| i.0.clone())
                                    .collect::<Vec<_>>()
                                    .join(".")
                            })
                            .collect();
                        reporter.emit(
                            Diagnostic::error(format!(
                                "deinit function {}.deinit({}) must not throw exceptions, but declares throws ({}); drops during unwinding would become undefined",
                                mname,
                                s.name.0,
                                throws_str.join(", ")
                            ))
                            .with_file(sf.path.clone()),
                        );
                    }
                }
            }
        }
    }

    // Provider field validation
    for d in &ast.decls {
        let AS::Decl::Provider(p) = d else { continue };

        // Duplicate field names
        let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for fld in &p.fields {
            if !seen.insert(&fld.name.0) {
                reporter.emit(
                    Diagnostic::error(format!(
                        "duplicate field '{}' in provider {}",
                        fld.name.0, p.name.0
                    ))
                    .with_file(sf.path.clone()),
                );
            }

            // Provider fields MUST use either 'final' or 'shared' modifier
            // Plain mutable fields are not allowed in providers (use shared state wrappers)
            if !fld.is_final && !fld.is_shared {
                reporter.emit(
                    Diagnostic::error(format!(
                        "provider field '{}' in '{}' must be 'final' or 'shared'; plain mutable fields are not allowed in providers. Use 'final' for immutable configuration or 'shared' with Shared<T>/Atomic<T>/Watch<T> for mutable state.",
                        fld.name.0, p.name.0
                    ))
                    .with_file(sf.path.clone()),
                );
            }

            // Fields cannot be both final and shared
            if fld.is_final && fld.is_shared {
                reporter.emit(
                    Diagnostic::error(format!(
                        "provider field '{}' in '{}' cannot be both 'final' and 'shared'; use one or the other",
                        fld.name.0, p.name.0
                    ))
                    .with_file(sf.path.clone()),
                );
            }

            // Field type must be known (primitive or declared symbol)
            let base_ty = map_name_to_ty(&fld.ty);
            let is_prim = !matches!(base_ty, Ty::Named(_) | Ty::Unknown);
            if !is_prim {
                let parts: Vec<String> = fld.ty.path.iter().map(|i| i.0.clone()).collect();
                if lookup_symbol_kind(rp, current_pkg, &parts).is_none() {
                    reporter.emit(
                        Diagnostic::error(format!(
                            "unknown field type '{}' in provider {}",
                            parts.join("."),
                            p.name.0
                        ))
                        .with_file(sf.path.clone()),
                    );
                }
            }

            // Shared fields should use appropriate wrapper types (Shared<T>, Atomic<T>, Watch<T>)
            if fld.is_shared {
                let type_name = fld.ty.path.first().map(|i| i.0.as_str()).unwrap_or("");
                if !matches!(
                    type_name,
                    "Shared" | "Atomic" | "Watch" | "Notify" | "Owned"
                ) {
                    reporter.emit(
                        Diagnostic::warning(format!(
                            "shared field '{}' in provider '{}' should use a concurrent wrapper type (Shared<T>, Atomic<T>, Watch<T>, Notify<T>); plain '{}' may cause data races",
                            fld.name.0, p.name.0, type_name
                        ))
                        .with_file(sf.path.clone()),
                    );
                }
            }
        }

        // Deinit constraint (soft): if provider has Shared/Atomic/Watch fields, recommend having a `deinit` function
        let needs_deinit = p.fields.iter().any(|f| {
            let b = f.ty.path.first().map(|i| i.0.as_str()).unwrap_or("");
            matches!(b, "Shared" | "Atomic" | "Watch" | "Notify")
        });
        if needs_deinit {
            let mname = format!("{}Fns", p.name.0);
            let key = (current_pkg.to_string(), mname.clone());
            let ok = module_index.get(&key).is_some_and(|sigs| {
                sigs.iter().any(|sig| {
                    sig.name.0 == "deinit"
                        && (sig
                            .params
                            .first()
                            .and_then(|pm| pm.ty.path.last().map(|i| &i.0))
                            == Some(&p.name.0))
                })
            });
            if !ok {
                reporter.emit(
                    Diagnostic::warning(format!(
                        "provider '{}' contains shared state fields; consider providing {}.deinit({}) to release resources",
                        p.name.0, mname, p.name.0
                    ))
                    .with_file(sf.path.clone()),
                );
            }
        }

        // Deinit must-not-throw constraint for providers
        let mname = format!("{}Fns", p.name.0);
        let key = (current_pkg.to_string(), mname.clone());
        if let Some(sigs) = module_index.get(&key) {
            for sig in sigs {
                if sig.name.0 == "deinit"
                    && sig
                        .params
                        .first()
                        .and_then(|pm| pm.ty.path.last().map(|i| &i.0))
                        == Some(&p.name.0)
                {
                    if !sig.throws.is_empty() {
                        let throws_str: Vec<String> = sig
                            .throws
                            .iter()
                            .map(|np| {
                                np.path
                                    .iter()
                                    .map(|i| i.0.clone())
                                    .collect::<Vec<_>>()
                                    .join(".")
                            })
                            .collect();
                        reporter.emit(
                            Diagnostic::error(format!(
                                "deinit function {}.deinit({}) must not throw exceptions, but declares throws ({})",
                                mname,
                                p.name.0,
                                throws_str.join(", ")
                            ))
                            .with_file(sf.path.clone()),
                        );
                    }
                }
            }
        }
    }
}

fn np_to_vec(np: &AS::NamePath) -> Vec<String> {
    np.path.iter().map(|i| i.0.clone()).collect()
}

fn type_text(np: &AS::NamePath) -> String {
    np_to_vec(np).join(".")
}

fn sig_key(sig: &AS::FuncSig) -> (String, Vec<String>, Option<String>) {
    let name = sig.name.0.clone();
    let params: Vec<String> = sig.params.iter().map(|p| type_text(&p.ty)).collect();
    let ret = sig.ret.as_ref().map(type_text);
    (name, params, ret)
}

fn is_prim_name(np: &AS::NamePath) -> bool {
    let base = np
        .path
        .last()
        .map(|i| i.0.to_ascii_lowercase())
        .unwrap_or_default();
    matches!(
        base.as_str(),
        "int" | "i32" | "i64" | "float" | "f32" | "f64" | "bool" | "char" | "string" | "bytes"
    )
}

/// Returns the concurrency traits (Sendable, Shareable) for a wrapper type.
/// Returns None if the type is not a recognized wrapper.
/// Returns the concurrency traits (Sendable, Shareable, UnwindSafe) for a wrapper type.
fn wrapper_conc_traits(np: &AS::NamePath) -> Option<(bool, bool, bool)> {
    match np.path.last().map(|i| i.0.as_str()) {
        // Shared: not sendable, shareable, NOT unwind-safe (interior mutability)
        Some("Shared") => Some((false, true, false)),
        // Watch: not sendable, shareable, NOT unwind-safe (interior mutability)
        Some("Watch") => Some((false, true, false)),
        // Atomic: sendable, shareable, unwind-safe (lock-free atomic ops)
        Some("Atomic") => Some((true, true, true)),
        // Notify: sendable, shareable, unwind-safe (no mutable state)
        Some("Notify") => Some((true, true, true)),
        // Owned: sendable, not shareable, unwind-safe
        Some("Owned") => Some((true, false, true)),
        _ => None,
    }
}

fn derive_concurrency_traits(
    files: &[(SourceFile, AS::FileAst)],
    _rp: &ResolvedProgram,
) -> ConcTraits {
    use std::collections::HashMap;

    // Helper to check concurrency traits for a single type (field or variant payload)
    // Returns (sendable, shareable) for this type.
    // pkg is the current package context for unqualified names.
    // Helper to check concurrency traits for a single type (field or variant payload)
    // Returns (Sendable, Shareable, UnwindSafe) for this type.
    fn check_type_conc(
        np: &AS::NamePath,
        pkg: &str,
        types: &HashMap<(String, String), (bool, bool, bool)>,
    ) -> (bool, bool, bool) {
        // Primitives are always Sendable, Shareable, and UnwindSafe
        if is_prim_name(np) {
            return (true, true, true);
        }
        // Check for wrapper types (Shared, Atomic, Watch, Notify, Owned)
        if let Some((s, sh, u)) = wrapper_conc_traits(np) {
            return (s, sh, u);
        }
        // Look up named type in our types map
        let parts = np_to_vec(np);
        let (dpkg, dname) = if parts.len() == 1 {
            (pkg.to_string(), parts[0].clone())
        } else {
            (
                parts[..parts.len() - 1].join("."),
                parts.last().unwrap().clone(),
            )
        };
        // If found in types map, use those values; otherwise default to (false, false, true)
        // Note: UnwindSafe defaults to true because most types are unwind-safe
        types
            .get(&(dpkg, dname))
            .cloned()
            .unwrap_or((false, false, true))
    }

    // Infer target type from module method signatures.
    // Looks at the first parameter of the first method to determine the target type.
    fn infer_target_type_from_methods(items: &[AS::FuncDecl]) -> Option<String> {
        for func in items {
            if let Some(first_param) = func.sig.params.first() {
                // Get the type name (last segment of the path)
                return first_param.ty.path.last().map(|i| i.0.clone());
            }
        }
        None
    }

    // Infer target type from module name using naming convention.
    // E.g., "PointFns" -> "Point", "StringOps" -> "String"
    fn infer_target_type_from_module_name(module_name: &str) -> Option<String> {
        for suffix in &["Fns", "Ops", "Impl", "Module"] {
            if module_name.ends_with(suffix) && module_name.len() > suffix.len() {
                return Some(module_name[..module_name.len() - suffix.len()].to_string());
            }
        }
        None
    }

    // Helper to check if a struct/enum has @notSendable attribute
    fn has_not_sendable_attr(attrs: &[AS::Attr]) -> bool {
        attrs.iter().any(|a| {
            a.name
                .path
                .last()
                .map(|i| i.0.as_str() == "notSendable")
                .unwrap_or(false)
        })
    }

    // Helper to check if a struct/enum has @notShareable attribute
    fn has_not_shareable_attr(attrs: &[AS::Attr]) -> bool {
        attrs.iter().any(|a| {
            a.name
                .path
                .last()
                .map(|i| i.0.as_str() == "notShareable")
                .unwrap_or(false)
        })
    }

    // Helper to check if a struct/enum has @notUnwindSafe attribute
    fn has_not_unwind_safe_attr(attrs: &[AS::Attr]) -> bool {
        attrs.iter().any(|a| {
            a.name
                .path
                .last()
                .map(|i| i.0.as_str() == "notUnwindSafe")
                .unwrap_or(false)
        })
    }

    // Track explicit opt-outs: (pkg, name) -> (notSendable, notShareable, notUnwindSafe)
    let mut opt_outs: HashMap<(String, String), (bool, bool, bool)> = HashMap::new();

    // Collect candidate named types (structs and enums) and check for opt-out attributes
    // (Sendable, Shareable, UnwindSafe) - UnwindSafe defaults to true
    let mut types: HashMap<(String, String), (bool, bool, bool)> = HashMap::new();
    for (_sf, ast) in files {
        if let Some(pkg) = pkg_string(ast) {
            for d in &ast.decls {
                match d {
                    AS::Decl::Struct(s) => {
                        let key = (pkg.clone(), s.name.0.clone());
                        // Default: not Sendable, not Shareable, IS UnwindSafe
                        types.entry(key.clone()).or_insert((false, false, true));
                        // Check for opt-out attributes
                        let not_send = has_not_sendable_attr(&s.attrs);
                        let not_share = has_not_shareable_attr(&s.attrs);
                        let not_unwind = has_not_unwind_safe_attr(&s.attrs);
                        if not_send || not_share || not_unwind {
                            opt_outs.insert(key, (not_send, not_share, not_unwind));
                        }
                    }
                    AS::Decl::Enum(e) => {
                        let key = (pkg.clone(), e.name.0.clone());
                        // Default: not Sendable, not Shareable, IS UnwindSafe
                        types.entry(key.clone()).or_insert((false, false, true));
                        // Check for opt-out attributes
                        let not_send = has_not_sendable_attr(&e.attrs);
                        let not_share = has_not_shareable_attr(&e.attrs);
                        let not_unwind = has_not_unwind_safe_attr(&e.attrs);
                        if not_send || not_share || not_unwind {
                            opt_outs.insert(key, (not_send, not_share, not_unwind));
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    // Phase 1: Collect explicit Sendable/Shareable implementations from module declarations
    // e.g., `module PointFns implements Sendable { ... }`
    // Target type is inferred from method signatures or module name convention
    for (_sf, ast) in files {
        let Some(pkg) = pkg_string(ast) else {
            continue;
        };
        for d in &ast.decls {
            if let AS::Decl::Module(m) = d {
                // Check if this module implements Sendable or Shareable
                let implements_sendable = m
                    .implements
                    .iter()
                    .any(|ifc| ifc.path.last().map(|i| i.0.as_str()) == Some("Sendable"));
                let implements_shareable = m
                    .implements
                    .iter()
                    .any(|ifc| ifc.path.last().map(|i| i.0.as_str()) == Some("Shareable"));

                if implements_sendable || implements_shareable {
                    // Try to infer target type:
                    // 1. First from method signatures (first parameter type)
                    // 2. Fall back to module name convention (e.g., PointFns -> Point)
                    let target_name = infer_target_type_from_methods(&m.items)
                        .or_else(|| infer_target_type_from_module_name(&m.name.0));
                    if let Some(name) = target_name {
                        if !name.is_empty() {
                            let key = (pkg.clone(), name);
                            let entry = types.entry(key).or_insert((false, false, true));
                            // Mark explicitly - explicit implementations take precedence
                            if implements_sendable {
                                entry.0 = true;
                            }
                            if implements_shareable {
                                entry.1 = true;
                            }
                        }
                    }
                }
            }
        }
    }

    // Phase 2: Fixed-point iteration to derive traits transitively from fields
    // Note: Structural derivation can only upgrade traits (never downgrade).
    // If explicit implementation set a trait to true, it stays true.
    // If structural derivation finds all fields are Sendable/Shareable, it sets to true.
    // However, @notSendable/@notShareable attributes block derivation.
    for _ in 0..8 {
        let mut changed = false;
        for (_sf, ast) in files {
            let Some(pkg) = pkg_string(ast) else {
                continue;
            };
            for d in &ast.decls {
                match d {
                    AS::Decl::Struct(s) => {
                        let key = (pkg.clone(), s.name.0.clone());

                        // Check for opt-out attributes
                        let (opt_out_send, opt_out_share, opt_out_unwind) =
                            opt_outs.get(&key).cloned().unwrap_or((false, false, false));

                        // A struct is Sendable if ALL fields are Sendable
                        // A struct is Shareable if ALL fields are Shareable AND ALL fields are final
                        // (C20: immutable data is Shareable, mutable structs are NOT Shareable)
                        let mut all_send = true;
                        let mut all_share = true;
                        let mut all_unwind = true;
                        let mut all_final = true;
                        for f in &s.fields {
                            let (fs, fsh, fu) = check_type_conc(&f.ty, &pkg, &types);
                            all_send &= fs;
                            all_share &= fsh;
                            all_unwind &= fu;
                            // C20: For Shareable, fields must also be immutable (final)
                            if !f.is_final {
                                all_final = false;
                            }
                        }
                        // C20: Shareable requires all fields to be final (immutable)
                        all_share = all_share && all_final;

                        // If opted out, don't derive that trait
                        if opt_out_send {
                            all_send = false;
                        }
                        if opt_out_share {
                            all_share = false;
                        }
                        if opt_out_unwind {
                            all_unwind = false;
                        }

                        let entry = types.entry(key).or_insert((false, false, true));
                        // Use OR to preserve explicit implementations (upgrade only, never downgrade)
                        // But opt-out blocks the upgrade
                        let new_send = if opt_out_send {
                            false
                        } else {
                            entry.0 || all_send
                        };
                        let new_share = if opt_out_share {
                            false
                        } else {
                            entry.1 || all_share
                        };
                        let new_unwind = if opt_out_unwind {
                            false
                        } else {
                            entry.2 && all_unwind
                        };
                        if entry.0 != new_send || entry.1 != new_share || entry.2 != new_unwind {
                            *entry = (new_send, new_share, new_unwind);
                            changed = true;
                        }
                    }
                    AS::Decl::Enum(e) => {
                        let key = (pkg.clone(), e.name.0.clone());

                        // Check for opt-out attributes
                        let (opt_out_send, opt_out_share, opt_out_unwind) =
                            opt_outs.get(&key).cloned().unwrap_or((false, false, false));

                        // An enum is Sendable if ALL variant payloads are Sendable
                        // An enum is Shareable if ALL variant payloads are Shareable
                        // Unit variants contribute (true, true) since they have no data
                        let mut all_send = true;
                        let mut all_share = true;
                        let mut all_unwind = true;
                        for v in &e.variants {
                            match v {
                                AS::EnumVariant::Unit { .. } => {
                                    // Unit variants have no data, always ok
                                }
                                AS::EnumVariant::Tuple {
                                    types: variant_types,
                                    ..
                                } => {
                                    for np in variant_types {
                                        let (ts, tsh, tu) = check_type_conc(np, &pkg, &types);
                                        all_send &= ts;
                                        all_share &= tsh;
                                        all_unwind &= tu;
                                    }
                                }
                            }
                        }

                        // If opted out, don't derive that trait
                        if opt_out_send {
                            all_send = false;
                        }
                        if opt_out_share {
                            all_share = false;
                        }
                        if opt_out_unwind {
                            all_unwind = false;
                        }

                        let entry = types.entry(key).or_insert((false, false, true));
                        // Use OR to preserve explicit implementations (upgrade only, never downgrade)
                        // But opt-out blocks the upgrade
                        let new_send = if opt_out_send {
                            false
                        } else {
                            entry.0 || all_send
                        };
                        let new_share = if opt_out_share {
                            false
                        } else {
                            entry.1 || all_share
                        };
                        let new_unwind = if opt_out_unwind {
                            false
                        } else {
                            entry.2 && all_unwind
                        };
                        if entry.0 != new_send || entry.1 != new_share || entry.2 != new_unwind {
                            *entry = (new_send, new_share, new_unwind);
                            changed = true;
                        }
                    }
                    _ => {}
                }
            }
        }
        if !changed {
            break;
        }
    }
    types
}

fn validate_interfaces(
    current_pkg: &str,
    ast: &AS::FileAst,
    rp: &ResolvedProgram,
    reporter: &mut Reporter,
    sf: &SourceFile,
) {
    use std::collections::HashSet;
    for d in &ast.decls {
        let AS::Decl::Interface(i) = d else { continue };
        // Check extends names resolve to interfaces
        for np in &i.extends {
            let parts = np_to_vec(np);
            match lookup_symbol_kind(rp, current_pkg, &parts) {
                Some(ResolvedKind::Interface) => {}
                Some(_) => reporter.emit(
                    Diagnostic::error(format!(
                        "interface '{}' extends non-interface '{}'",
                        i.name.0,
                        parts.join(".")
                    ))
                    .with_file(sf.path.clone()),
                ),
                None => reporter.emit(
                    Diagnostic::error(format!(
                        "interface '{}' extends unknown '{}'",
                        i.name.0,
                        parts.join(".")
                    ))
                    .with_file(sf.path.clone()),
                ),
            }
        }
        // Duplicate method names within same interface
        let mut seen: HashSet<&str> = HashSet::new();
        for m in &i.methods {
            if !seen.insert(&m.sig.name.0) {
                reporter.emit(
                    Diagnostic::error(format!(
                        "duplicate method '{}' in interface {}",
                        m.sig.name.0, i.name.0
                    ))
                    .with_file(sf.path.clone()),
                );
            }
        }
    }
}

fn validate_enums(
    current_pkg: &str,
    ast: &AS::FileAst,
    rp: &ResolvedProgram,
    reporter: &mut Reporter,
    sf: &SourceFile,
) {
    use std::collections::HashSet;
    for d in &ast.decls {
        let AS::Decl::Enum(e) = d else { continue };
        // Collect this enum's generic parameter names to allow them in payloads.
        let generic_names: std::collections::HashSet<String> =
            e.generics.iter().map(|g| g.name.0.clone()).collect();
        // Duplicate variant names
        let mut seen: HashSet<&str> = HashSet::new();
        for v in &e.variants {
            match v {
                AS::EnumVariant::Unit { name, .. } => {
                    if !seen.insert(&name.0) {
                        reporter.emit(
                            Diagnostic::error(format!(
                                "duplicate enum variant '{}' in {}",
                                name.0, e.name.0
                            ))
                            .with_file(sf.path.clone()),
                        );
                    }
                }
                AS::EnumVariant::Tuple {
                    name, types: tys, ..
                } => {
                    if !seen.insert(&name.0) {
                        reporter.emit(
                            Diagnostic::error(format!(
                                "duplicate enum variant '{}' in {}",
                                name.0, e.name.0
                            ))
                            .with_file(sf.path.clone()),
                        );
                    }
                    // Validate payload types exist
                    for ty in tys {
                        let base = map_name_to_ty(ty);
                        let is_prim = !matches!(base, Ty::Named(_) | Ty::Unknown);
                        if !is_prim {
                            let parts: Vec<String> = ty.path.iter().map(|i| i.0.clone()).collect();
                            // Allow generic parameters declared on the enum itself (e.g., Result<T,E> variants use T/E)
                            if parts.len() == 1 && generic_names.contains(&parts[0]) {
                                continue;
                            }
                            if lookup_symbol_kind(rp, current_pkg, &parts).is_none() {
                                reporter.emit(
                                    Diagnostic::error(format!(
                                        "unknown payload type '{}' for variant '{}.{}'",
                                        parts.join("."),
                                        e.name.0,
                                        name.0
                                    ))
                                    .with_file(sf.path.clone()),
                                );
                            }
                        }
                    }
                }
            }
        }
        // Optional sealed validation: ensure sealed enums have at least one variant
        if e.is_sealed && e.variants.is_empty() {
            reporter.emit(
                Diagnostic::error(format!(
                    "sealed enum '{}' must declare at least one variant",
                    e.name.0
                ))
                .with_file(sf.path.clone()),
            );
        }
    }
}

fn throws_list_text(sig: &AS::FuncSig) -> Vec<String> {
    sig.throws
        .iter()
        .map(|np| {
            np.path
                .iter()
                .map(|i| i.0.clone())
                .collect::<Vec<_>>()
                .join(".")
        })
        .collect()
}

fn validate_exceptions(
    current_pkg: &str,
    ast: &AS::FileAst,
    rp: &ResolvedProgram,
    reporter: &mut Reporter,
    sf: &SourceFile,
    module_index: &std::collections::HashMap<(String, String), Vec<AS::FuncSig>>,
) {
    // Helper to look up what exceptions a function call might throw
    fn lookup_callee_throws(
        callee: &AS::Expr,
        current_pkg: &str,
        module_index: &std::collections::HashMap<(String, String), Vec<AS::FuncSig>>,
    ) -> Vec<String> {
        // Extract Module.method pattern
        if let AS::Expr::Member(obj, method) = callee {
            if let AS::Expr::Ident(module_name) = &**obj {
                // Try current package first
                let key = (current_pkg.to_string(), module_name.0.clone());
                if let Some(sigs) = module_index.get(&key) {
                    for sig in sigs {
                        if sig.name.0 == method.0 {
                            return sig
                                .throws
                                .iter()
                                .map(|np| {
                                    np.path
                                        .iter()
                                        .map(|i| i.0.clone())
                                        .collect::<Vec<_>>()
                                        .join(".")
                                })
                                .collect();
                        }
                    }
                }
                // Try other packages (simplified: just check common patterns)
                for ((pkg, mod_name), sigs) in module_index.iter() {
                    if mod_name == &module_name.0 {
                        for sig in sigs {
                            if sig.name.0 == method.0 {
                                return sig
                                    .throws
                                    .iter()
                                    .map(|np| {
                                        np.path
                                            .iter()
                                            .map(|i| i.0.clone())
                                            .collect::<Vec<_>>()
                                            .join(".")
                                    })
                                    .collect();
                            }
                        }
                        let _ = pkg; // silence unused warning
                    }
                }
            }
        }

        // Handle direct function calls (within same module): fn_name()
        if let AS::Expr::Ident(func_name) = callee {
            // Search all modules in current package for this function
            for ((pkg, _mod_name), sigs) in module_index.iter() {
                if pkg == current_pkg {
                    for sig in sigs {
                        if sig.name.0 == func_name.0 {
                            return sig
                                .throws
                                .iter()
                                .map(|np| {
                                    np.path
                                        .iter()
                                        .map(|i| i.0.clone())
                                        .collect::<Vec<_>>()
                                        .join(".")
                                })
                                .collect();
                        }
                    }
                }
            }
        }

        Vec::new()
    }

    // Helper to collect all exception types thrown in a block (for catch exhaustiveness)
    fn collect_thrown_types_in_block(
        b: &AS::Block,
        current_pkg: &str,
        module_index: &std::collections::HashMap<(String, String), Vec<AS::FuncSig>>,
        out: &mut std::collections::HashSet<String>,
    ) {
        fn collect_from_expr(
            e: &AS::Expr,
            current_pkg: &str,
            module_index: &std::collections::HashMap<(String, String), Vec<AS::FuncSig>>,
            out: &mut std::collections::HashSet<String>,
        ) {
            match e {
                AS::Expr::Call(callee, args) => {
                    let throws = lookup_callee_throws(callee, current_pkg, module_index);
                    for t in throws {
                        out.insert(t);
                    }
                    collect_from_expr(callee, current_pkg, module_index, out);
                    for a in args {
                        collect_from_expr(a, current_pkg, module_index, out);
                    }
                }
                AS::Expr::Binary(l, _, r) => {
                    collect_from_expr(l, current_pkg, module_index, out);
                    collect_from_expr(r, current_pkg, module_index, out);
                }
                AS::Expr::Unary(_, inner) => {
                    collect_from_expr(inner, current_pkg, module_index, out);
                }
                AS::Expr::Ternary(c, t, f) => {
                    collect_from_expr(c, current_pkg, module_index, out);
                    collect_from_expr(t, current_pkg, module_index, out);
                    collect_from_expr(f, current_pkg, module_index, out);
                }
                AS::Expr::Member(obj, _) => {
                    collect_from_expr(obj, current_pkg, module_index, out);
                }
                AS::Expr::Index(obj, idx) => {
                    collect_from_expr(obj, current_pkg, module_index, out);
                    collect_from_expr(idx, current_pkg, module_index, out);
                }
                AS::Expr::Await(inner) | AS::Expr::Cast(_, inner) => {
                    collect_from_expr(inner, current_pkg, module_index, out);
                }
                AS::Expr::ListLit(items) => {
                    for it in items {
                        collect_from_expr(it, current_pkg, module_index, out);
                    }
                }
                AS::Expr::MapLit { pairs, spread } => {
                    for (k, v) in pairs {
                        collect_from_expr(k, current_pkg, module_index, out);
                        collect_from_expr(v, current_pkg, module_index, out);
                    }
                    if let Some(s) = spread {
                        collect_from_expr(s, current_pkg, module_index, out);
                    }
                }
                AS::Expr::StructLit { fields, spread, .. } => {
                    for (_, v) in fields {
                        collect_from_expr(v, current_pkg, module_index, out);
                    }
                    if let Some(s) = spread {
                        collect_from_expr(s, current_pkg, module_index, out);
                    }
                }
                _ => {}
            }
        }

        for st in &b.stmts {
            match st {
                AS::Stmt::Throw(e) => {
                    // Extract thrown type from expression
                    let thrown_type = match e {
                        AS::Expr::Call(callee, _) => {
                            if let AS::Expr::Member(obj, _) = &**callee {
                                if let AS::Expr::Ident(id) = &**obj {
                                    Some(id.0.clone())
                                } else {
                                    None
                                }
                            } else if let AS::Expr::Ident(id) = &**callee {
                                Some(id.0.clone())
                            } else {
                                None
                            }
                        }
                        AS::Expr::StructLit { type_name, .. } => {
                            if let AS::Expr::Ident(id) = &**type_name {
                                Some(id.0.clone())
                            } else if let AS::Expr::Member(_, name) = &**type_name {
                                Some(name.0.clone())
                            } else {
                                None
                            }
                        }
                        AS::Expr::Ident(id) => Some(id.0.clone()),
                        _ => None,
                    };
                    if let Some(ty) = thrown_type {
                        out.insert(ty);
                    }
                }
                AS::Stmt::Expr(e) => collect_from_expr(e, current_pkg, module_index, out),
                AS::Stmt::VarDecl { init, .. } => {
                    if let Some(e) = init {
                        collect_from_expr(e, current_pkg, module_index, out);
                    }
                }
                AS::Stmt::Assign { expr, .. } => {
                    collect_from_expr(expr, current_pkg, module_index, out)
                }
                AS::Stmt::FieldAssign { expr, .. } => {
                    collect_from_expr(expr, current_pkg, module_index, out)
                }
                AS::Stmt::AssignOp { expr, .. } => {
                    collect_from_expr(expr, current_pkg, module_index, out)
                }
                AS::Stmt::Return(opt_e) => {
                    if let Some(e) = opt_e {
                        collect_from_expr(e, current_pkg, module_index, out);
                    }
                }
                AS::Stmt::If {
                    cond,
                    then_blk,
                    else_blk,
                    ..
                } => {
                    collect_from_expr(cond, current_pkg, module_index, out);
                    collect_thrown_types_in_block(then_blk, current_pkg, module_index, out);
                    if let Some(eb) = else_blk {
                        collect_thrown_types_in_block(eb, current_pkg, module_index, out);
                    }
                }
                AS::Stmt::While { cond, body, .. } => {
                    collect_from_expr(cond, current_pkg, module_index, out);
                    collect_thrown_types_in_block(body, current_pkg, module_index, out);
                }
                AS::Stmt::For {
                    init,
                    cond,
                    step,
                    body,
                    ..
                } => {
                    if let Some(init_stmt) = init {
                        // Init is a statement, but may contain expressions
                        match init_stmt.as_ref() {
                            AS::Stmt::VarDecl { init: Some(e), .. } => {
                                collect_from_expr(e, current_pkg, module_index, out);
                            }
                            AS::Stmt::Assign { expr, .. } => {
                                collect_from_expr(expr, current_pkg, module_index, out);
                            }
                            AS::Stmt::Expr(e) => {
                                collect_from_expr(e, current_pkg, module_index, out);
                            }
                            _ => {}
                        }
                    }
                    if let Some(c) = cond {
                        collect_from_expr(c, current_pkg, module_index, out);
                    }
                    if let Some(step_stmt) = step {
                        match step_stmt.as_ref() {
                            AS::Stmt::Assign { expr, .. } => {
                                collect_from_expr(expr, current_pkg, module_index, out);
                            }
                            AS::Stmt::AssignOp { expr, .. } => {
                                collect_from_expr(expr, current_pkg, module_index, out);
                            }
                            AS::Stmt::Expr(e) => {
                                collect_from_expr(e, current_pkg, module_index, out);
                            }
                            _ => {}
                        }
                    }
                    collect_thrown_types_in_block(body, current_pkg, module_index, out);
                }
                AS::Stmt::Block(b2) => {
                    collect_thrown_types_in_block(b2, current_pkg, module_index, out)
                }
                AS::Stmt::Unsafe(b2) => {
                    collect_thrown_types_in_block(b2, current_pkg, module_index, out)
                }
                AS::Stmt::Switch {
                    expr,
                    cases,
                    pattern_cases,
                    default,
                } => {
                    collect_from_expr(expr, current_pkg, module_index, out);
                    for (_, blk) in cases {
                        collect_thrown_types_in_block(blk, current_pkg, module_index, out);
                    }
                    for (_, blk) in pattern_cases {
                        collect_thrown_types_in_block(blk, current_pkg, module_index, out);
                    }
                    if let Some(db) = default {
                        collect_thrown_types_in_block(db, current_pkg, module_index, out);
                    }
                }
                // Try blocks: collect exceptions that escape the try/catch
                // Exceptions thrown in the try block are removed if caught by a catch handler
                AS::Stmt::Try {
                    try_blk,
                    catches,
                    finally_blk,
                } => {
                    // Collect what's thrown in the inner try block
                    let mut inner_thrown: std::collections::HashSet<String> =
                        std::collections::HashSet::new();
                    collect_thrown_types_in_block(
                        try_blk,
                        current_pkg,
                        module_index,
                        &mut inner_thrown,
                    );

                    // Collect what's caught by the catch handlers
                    let mut inner_caught: std::collections::HashSet<String> =
                        std::collections::HashSet::new();
                    let mut has_bare_catch = false;
                    for c in catches {
                        if let Some(ty) = &c.ty {
                            let tname: String = ty
                                .path
                                .iter()
                                .map(|i| i.0.clone())
                                .collect::<Vec<_>>()
                                .join(".");
                            inner_caught.insert(tname);
                        } else {
                            has_bare_catch = true;
                        }
                    }

                    // Only propagate exceptions not caught locally
                    if !has_bare_catch && !inner_caught.contains("Error") {
                        for thrown in &inner_thrown {
                            if !inner_caught.contains(thrown) {
                                out.insert(thrown.clone());
                            }
                        }
                    }
                    // If there's a bare catch or catch(Error), nothing escapes from try block

                    // Also collect exceptions thrown in catch handlers (they escape)
                    for c in catches {
                        collect_thrown_types_in_block(&c.blk, current_pkg, module_index, out);
                    }

                    // And finally block
                    if let Some(fb) = finally_blk {
                        collect_thrown_types_in_block(fb, current_pkg, module_index, out);
                    }
                }
                _ => {}
            }
        }
    }

    // Helper to collect all function calls in an expression and check their throws
    fn check_expr_throws(
        e: &AS::Expr,
        caught_types: &std::collections::HashSet<String>,
        declared_throws: &[String],
        sf: &SourceFile,
        reporter: &mut Reporter,
        current_pkg: &str,
        module_index: &std::collections::HashMap<(String, String), Vec<AS::FuncSig>>,
    ) {
        match e {
            AS::Expr::Call(callee, args) => {
                // Check what the callee throws
                let callee_throws = lookup_callee_throws(callee, current_pkg, module_index);
                for throw_type in &callee_throws {
                    // Check if this exception type is caught or declared
                    let is_caught =
                        caught_types.contains(throw_type) || caught_types.contains("Error"); // Error is wildcard
                    let is_declared = declared_throws.contains(throw_type)
                        || declared_throws.iter().any(|d| d == "Error");

                    if !is_caught && !is_declared {
                        reporter.emit(
                            Diagnostic::error(format!(
                                "call to function that throws '{}' must be caught or declared in throws clause",
                                throw_type
                            ))
                            .with_file(sf.path.clone()),
                        );
                    }
                }
                // Recurse into callee and args
                check_expr_throws(
                    callee,
                    caught_types,
                    declared_throws,
                    sf,
                    reporter,
                    current_pkg,
                    module_index,
                );
                for arg in args {
                    check_expr_throws(
                        arg,
                        caught_types,
                        declared_throws,
                        sf,
                        reporter,
                        current_pkg,
                        module_index,
                    );
                }
            }
            AS::Expr::Binary(l, _, r) => {
                check_expr_throws(
                    l,
                    caught_types,
                    declared_throws,
                    sf,
                    reporter,
                    current_pkg,
                    module_index,
                );
                check_expr_throws(
                    r,
                    caught_types,
                    declared_throws,
                    sf,
                    reporter,
                    current_pkg,
                    module_index,
                );
            }
            AS::Expr::Unary(_, inner) => {
                check_expr_throws(
                    inner,
                    caught_types,
                    declared_throws,
                    sf,
                    reporter,
                    current_pkg,
                    module_index,
                );
            }
            AS::Expr::Ternary(c, t, f) => {
                check_expr_throws(
                    c,
                    caught_types,
                    declared_throws,
                    sf,
                    reporter,
                    current_pkg,
                    module_index,
                );
                check_expr_throws(
                    t,
                    caught_types,
                    declared_throws,
                    sf,
                    reporter,
                    current_pkg,
                    module_index,
                );
                check_expr_throws(
                    f,
                    caught_types,
                    declared_throws,
                    sf,
                    reporter,
                    current_pkg,
                    module_index,
                );
            }
            AS::Expr::Member(obj, _) => {
                check_expr_throws(
                    obj,
                    caught_types,
                    declared_throws,
                    sf,
                    reporter,
                    current_pkg,
                    module_index,
                );
            }
            AS::Expr::OptionalMember(obj, _) => {
                check_expr_throws(
                    obj,
                    caught_types,
                    declared_throws,
                    sf,
                    reporter,
                    current_pkg,
                    module_index,
                );
            }
            AS::Expr::Index(obj, idx) => {
                check_expr_throws(
                    obj,
                    caught_types,
                    declared_throws,
                    sf,
                    reporter,
                    current_pkg,
                    module_index,
                );
                check_expr_throws(
                    idx,
                    caught_types,
                    declared_throws,
                    sf,
                    reporter,
                    current_pkg,
                    module_index,
                );
            }
            AS::Expr::ListLit(elems) => {
                for elem in elems {
                    check_expr_throws(
                        elem,
                        caught_types,
                        declared_throws,
                        sf,
                        reporter,
                        current_pkg,
                        module_index,
                    );
                }
            }
            AS::Expr::MapLit { pairs, spread } => {
                for (k, v) in pairs {
                    check_expr_throws(
                        k,
                        caught_types,
                        declared_throws,
                        sf,
                        reporter,
                        current_pkg,
                        module_index,
                    );
                    check_expr_throws(
                        v,
                        caught_types,
                        declared_throws,
                        sf,
                        reporter,
                        current_pkg,
                        module_index,
                    );
                }
                if let Some(s) = spread {
                    check_expr_throws(
                        s,
                        caught_types,
                        declared_throws,
                        sf,
                        reporter,
                        current_pkg,
                        module_index,
                    );
                }
            }
            AS::Expr::StructLit { fields, spread, .. } => {
                for (_, v) in fields {
                    check_expr_throws(
                        v,
                        caught_types,
                        declared_throws,
                        sf,
                        reporter,
                        current_pkg,
                        module_index,
                    );
                }
                if let Some(s) = spread {
                    check_expr_throws(
                        s,
                        caught_types,
                        declared_throws,
                        sf,
                        reporter,
                        current_pkg,
                        module_index,
                    );
                }
            }
            AS::Expr::Await(inner) => {
                check_expr_throws(
                    inner,
                    caught_types,
                    declared_throws,
                    sf,
                    reporter,
                    current_pkg,
                    module_index,
                );
            }
            AS::Expr::Cast(_, inner) => {
                check_expr_throws(
                    inner,
                    caught_types,
                    declared_throws,
                    sf,
                    reporter,
                    current_pkg,
                    module_index,
                );
            }
            // Leaf expressions - no throws to check
            AS::Expr::Int(_)
            | AS::Expr::Float(_)
            | AS::Expr::Str(_)
            | AS::Expr::Char(_)
            | AS::Expr::Bool(_)
            | AS::Expr::Ident(_) => {}
            // FnLiteral body is checked separately
            AS::Expr::FnLiteral(_, _) => {}
        }
    }

    // Walk a block checking for throw statements and function calls
    // exc_var_types maps exception variable names to their types for rethrow checking
    fn walk_block(
        b: &AS::Block,
        declared_throws: &[String],
        caught_types: &std::collections::HashSet<String>,
        exc_var_types: &std::collections::HashMap<String, String>,
        sf: &SourceFile,
        reporter: &mut Reporter,
        current_pkg: &str,
        module_index: &std::collections::HashMap<(String, String), Vec<AS::FuncSig>>,
        rp: &ResolvedProgram,
    ) {
        for st in &b.stmts {
            match st {
                AS::Stmt::Try {
                    try_blk,
                    catches,
                    finally_blk,
                } => {
                    // Validate that catch clause exception types exist
                    let mut local_caught: std::collections::HashSet<String> = caught_types.clone();
                    let mut has_bare_catch = false;
                    for c in catches {
                        if let Some(ty) = &c.ty {
                            let parts: Vec<String> = ty.path.iter().map(|i| i.0.clone()).collect();
                            let tname = parts.join(".");

                            // Validate catch type exists (allow "Error" as special base type)
                            if !(parts.len() == 1 && parts[0] == "Error") {
                                if lookup_symbol_kind(rp, current_pkg, &parts).is_none() {
                                    reporter.emit(
                                        Diagnostic::error(format!(
                                            "unknown exception type '{}' in catch clause",
                                            tname
                                        ))
                                        .with_file(sf.path.clone()),
                                    );
                                }
                            }
                            local_caught.insert(tname);
                        } else {
                            // Bare catch (no type) catches everything
                            has_bare_catch = true;
                        }
                    }

                    // Collect exceptions thrown in the try block for exhaustiveness checking
                    let mut try_thrown_types: std::collections::HashSet<String> =
                        std::collections::HashSet::new();
                    collect_thrown_types_in_block(
                        try_blk,
                        current_pkg,
                        module_index,
                        &mut try_thrown_types,
                    );

                    // Check catch exhaustiveness: all thrown exceptions should be caught
                    // unless they're declared in the function's throws clause or there's a bare catch
                    if !has_bare_catch && !local_caught.contains("Error") {
                        for thrown in &try_thrown_types {
                            let is_caught = local_caught.contains(thrown);
                            let is_declared = declared_throws.contains(thrown)
                                || declared_throws.iter().any(|d| d == "Error");

                            if !is_caught && !is_declared {
                                reporter.emit(
                                    Diagnostic::error(format!(
                                        "exception '{}' thrown in try block is not caught; add 'catch ({} e)' or declare in throws clause",
                                        thrown, thrown
                                    ))
                                    .with_file(sf.path.clone()),
                                );
                            }
                        }
                    }

                    // Walk try block with the caught types added to context
                    walk_block(
                        try_blk,
                        declared_throws,
                        &local_caught,
                        exc_var_types,
                        sf,
                        reporter,
                        current_pkg,
                        module_index,
                        rp,
                    );
                    // Walk catch blocks with original caught_types (not the local ones)
                    // but with the exception variable mapped to its type
                    for c in catches {
                        let mut catch_var_types = exc_var_types.clone();
                        if let Some(var) = &c.var {
                            if let Some(ty) = &c.ty {
                                let tname: String = ty
                                    .path
                                    .iter()
                                    .map(|i| i.0.clone())
                                    .collect::<Vec<_>>()
                                    .join(".");
                                catch_var_types.insert(var.0.clone(), tname);
                            }
                        }
                        walk_block(
                            &c.blk,
                            declared_throws,
                            caught_types,
                            &catch_var_types,
                            sf,
                            reporter,
                            current_pkg,
                            module_index,
                            rp,
                        );
                    }
                    // Walk finally block with original caught_types
                    if let Some(fb) = finally_blk {
                        walk_block(
                            fb,
                            declared_throws,
                            caught_types,
                            exc_var_types,
                            sf,
                            reporter,
                            current_pkg,
                            module_index,
                            rp,
                        );
                    }
                }
                AS::Stmt::If {
                    cond,
                    then_blk,
                    else_blk,
                    ..
                } => {
                    check_expr_throws(
                        cond,
                        caught_types,
                        declared_throws,
                        sf,
                        reporter,
                        current_pkg,
                        module_index,
                    );
                    walk_block(
                        then_blk,
                        declared_throws,
                        caught_types,
                        exc_var_types,
                        sf,
                        reporter,
                        current_pkg,
                        module_index,
                        rp,
                    );
                    if let Some(b) = else_blk {
                        walk_block(
                            b,
                            declared_throws,
                            caught_types,
                            exc_var_types,
                            sf,
                            reporter,
                            current_pkg,
                            module_index,
                            rp,
                        );
                    }
                }
                AS::Stmt::While { cond, body, .. } => {
                    check_expr_throws(
                        cond,
                        caught_types,
                        declared_throws,
                        sf,
                        reporter,
                        current_pkg,
                        module_index,
                    );
                    walk_block(
                        body,
                        declared_throws,
                        caught_types,
                        exc_var_types,
                        sf,
                        reporter,
                        current_pkg,
                        module_index,
                        rp,
                    );
                }
                AS::Stmt::For { cond, body, .. } => {
                    if let Some(c) = cond {
                        check_expr_throws(
                            c,
                            caught_types,
                            declared_throws,
                            sf,
                            reporter,
                            current_pkg,
                            module_index,
                        );
                    }
                    walk_block(
                        body,
                        declared_throws,
                        caught_types,
                        exc_var_types,
                        sf,
                        reporter,
                        current_pkg,
                        module_index,
                        rp,
                    );
                }
                AS::Stmt::Labeled { stmt, .. } => {
                    // Recurse into labeled inner statement
                    match &**stmt {
                        AS::Stmt::Block(b) => walk_block(
                            b,
                            declared_throws,
                            caught_types,
                            exc_var_types,
                            sf,
                            reporter,
                            current_pkg,
                            module_index,
                            rp,
                        ),
                        AS::Stmt::Unsafe(b) => walk_block(
                            b,
                            declared_throws,
                            caught_types,
                            exc_var_types,
                            sf,
                            reporter,
                            current_pkg,
                            module_index,
                            rp,
                        ),
                        AS::Stmt::If {
                            cond,
                            then_blk,
                            else_blk,
                            ..
                        } => {
                            check_expr_throws(
                                cond,
                                caught_types,
                                declared_throws,
                                sf,
                                reporter,
                                current_pkg,
                                module_index,
                            );
                            walk_block(
                                then_blk,
                                declared_throws,
                                caught_types,
                                exc_var_types,
                                sf,
                                reporter,
                                current_pkg,
                                module_index,
                                rp,
                            );
                            if let Some(b) = else_blk {
                                walk_block(
                                    b,
                                    declared_throws,
                                    caught_types,
                                    exc_var_types,
                                    sf,
                                    reporter,
                                    current_pkg,
                                    module_index,
                                    rp,
                                );
                            }
                        }
                        AS::Stmt::While { cond, body, .. } => {
                            check_expr_throws(
                                cond,
                                caught_types,
                                declared_throws,
                                sf,
                                reporter,
                                current_pkg,
                                module_index,
                            );
                            walk_block(
                                body,
                                declared_throws,
                                caught_types,
                                exc_var_types,
                                sf,
                                reporter,
                                current_pkg,
                                module_index,
                                rp,
                            );
                        }
                        AS::Stmt::For { cond, body, .. } => {
                            if let Some(c) = cond {
                                check_expr_throws(
                                    c,
                                    caught_types,
                                    declared_throws,
                                    sf,
                                    reporter,
                                    current_pkg,
                                    module_index,
                                );
                            }
                            walk_block(
                                body,
                                declared_throws,
                                caught_types,
                                exc_var_types,
                                sf,
                                reporter,
                                current_pkg,
                                module_index,
                                rp,
                            );
                        }
                        AS::Stmt::Switch {
                            expr,
                            cases,
                            default,
                            ..
                        } => {
                            check_expr_throws(
                                expr,
                                caught_types,
                                declared_throws,
                                sf,
                                reporter,
                                current_pkg,
                                module_index,
                            );
                            for (_, blk) in cases {
                                walk_block(
                                    blk,
                                    declared_throws,
                                    caught_types,
                                    exc_var_types,
                                    sf,
                                    reporter,
                                    current_pkg,
                                    module_index,
                                    rp,
                                );
                            }
                            if let Some(db) = default {
                                walk_block(
                                    db,
                                    declared_throws,
                                    caught_types,
                                    exc_var_types,
                                    sf,
                                    reporter,
                                    current_pkg,
                                    module_index,
                                    rp,
                                );
                            }
                        }
                        _ => {}
                    }
                }
                AS::Stmt::Switch {
                    expr,
                    cases,
                    default,
                    ..
                } => {
                    check_expr_throws(
                        expr,
                        caught_types,
                        declared_throws,
                        sf,
                        reporter,
                        current_pkg,
                        module_index,
                    );
                    for (_, blk) in cases {
                        walk_block(
                            blk,
                            declared_throws,
                            caught_types,
                            exc_var_types,
                            sf,
                            reporter,
                            current_pkg,
                            module_index,
                            rp,
                        );
                    }
                    if let Some(db) = default {
                        walk_block(
                            db,
                            declared_throws,
                            caught_types,
                            exc_var_types,
                            sf,
                            reporter,
                            current_pkg,
                            module_index,
                            rp,
                        );
                    }
                }
                AS::Stmt::Block(b) => walk_block(
                    b,
                    declared_throws,
                    caught_types,
                    exc_var_types,
                    sf,
                    reporter,
                    current_pkg,
                    module_index,
                    rp,
                ),
                AS::Stmt::Unsafe(b) => walk_block(
                    b,
                    declared_throws,
                    caught_types,
                    exc_var_types,
                    sf,
                    reporter,
                    current_pkg,
                    module_index,
                    rp,
                ),
                AS::Stmt::Throw(e) => {
                    // Check that the thrown type is declared in the function's throws clause
                    // Extract the type name from the expression if possible
                    let thrown_type = match e {
                        AS::Expr::Call(callee, _) => {
                            // Constructor call like ErrorType.new(...) or Module.ErrorType.new(...)
                            if let AS::Expr::Member(obj, _method) = &**callee {
                                if let AS::Expr::Ident(id) = &**obj {
                                    Some(id.0.clone())
                                } else if let AS::Expr::Member(inner_obj, type_name) = &**obj {
                                    // pkg.ErrorType.new(...)
                                    if let AS::Expr::Ident(_) = &**inner_obj {
                                        Some(type_name.0.clone())
                                    } else {
                                        None
                                    }
                                } else {
                                    None
                                }
                            } else if let AS::Expr::Ident(id) = &**callee {
                                Some(id.0.clone())
                            } else {
                                None
                            }
                        }
                        AS::Expr::StructLit { type_name, .. } => {
                            // Struct literal: ErrorType { message: "..." }
                            // type_name is Box<Expr>, typically Ident or Member
                            if let AS::Expr::Ident(id) = &**type_name {
                                Some(id.0.clone())
                            } else if let AS::Expr::Member(_, name) = &**type_name {
                                Some(name.0.clone())
                            } else {
                                None
                            }
                        }
                        AS::Expr::Ident(id) => {
                            // For identifiers, check if this is a caught exception variable
                            // If so, use its exception type; otherwise use the identifier name
                            if let Some(exc_type) = exc_var_types.get(&id.0) {
                                Some(exc_type.clone())
                            } else {
                                Some(id.0.clone())
                            }
                        }
                        _ => None,
                    };
                    if let Some(ty_name) = thrown_type {
                        // Check if this type is caught locally or declared in throws
                        let is_caught =
                            caught_types.contains(&ty_name) || caught_types.contains("Error");
                        let is_declared = declared_throws.contains(&ty_name)
                            || declared_throws.iter().any(|d| d == "Error");

                        if !is_caught && !is_declared {
                            // Not caught and not declared - error
                            if declared_throws.is_empty() {
                                // Function doesn't declare any throws - error
                                reporter.emit(
                                    Diagnostic::error(format!(
                                        "throw statement in function that does not declare throws; add 'throws ({})' to the function signature",
                                        ty_name
                                    ))
                                    .with_file(sf.path.clone()),
                                );
                            } else {
                                // Function declares throws but this type isn't in the list
                                reporter.emit(
                                    Diagnostic::error(format!(
                                        "thrown type '{}' is not declared in function's throws clause; add '{}' to the throws list",
                                        ty_name, ty_name
                                    ))
                                    .with_file(sf.path.clone()),
                                );
                            }
                        }
                    }
                }
                AS::Stmt::Expr(e) => {
                    check_expr_throws(
                        e,
                        caught_types,
                        declared_throws,
                        sf,
                        reporter,
                        current_pkg,
                        module_index,
                    );
                }
                AS::Stmt::VarDecl { init, .. } => {
                    if let Some(e) = init {
                        check_expr_throws(
                            e,
                            caught_types,
                            declared_throws,
                            sf,
                            reporter,
                            current_pkg,
                            module_index,
                        );
                    }
                }
                AS::Stmt::Assign { expr, .. } => {
                    check_expr_throws(
                        expr,
                        caught_types,
                        declared_throws,
                        sf,
                        reporter,
                        current_pkg,
                        module_index,
                    );
                }
                AS::Stmt::Return(opt_e) => {
                    if let Some(e) = opt_e {
                        check_expr_throws(
                            e,
                            caught_types,
                            declared_throws,
                            sf,
                            reporter,
                            current_pkg,
                            module_index,
                        );
                    }
                }
                AS::Stmt::FieldAssign { expr, .. } => {
                    check_expr_throws(
                        expr,
                        caught_types,
                        declared_throws,
                        sf,
                        reporter,
                        current_pkg,
                        module_index,
                    );
                }
                AS::Stmt::AssignOp { expr, .. } => {
                    check_expr_throws(
                        expr,
                        caught_types,
                        declared_throws,
                        sf,
                        reporter,
                        current_pkg,
                        module_index,
                    );
                }
                AS::Stmt::Break(_) | AS::Stmt::Continue(_) => {}
                AS::Stmt::PrintStr(_)
                | AS::Stmt::PrintExpr(_)
                | AS::Stmt::PrintRawStr(_)
                | AS::Stmt::PrintRawExpr(_)
                | AS::Stmt::Panic(_) => {}
            }
        }
    }

    // Validate throws exist and walk function bodies
    let empty_caught: std::collections::HashSet<String> = std::collections::HashSet::new();
    let empty_exc_vars: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    for d in &ast.decls {
        match d {
            AS::Decl::Function(f) => {
                for np in &f.sig.throws {
                    let parts: Vec<String> = np.path.iter().map(|i| i.0.clone()).collect();
                    if parts.len() == 1 && parts[0] == "Error" {
                        continue;
                    }
                    if lookup_symbol_kind(rp, current_pkg, &parts).is_none() {
                        reporter.emit(
                            Diagnostic::error(format!(
                                "unknown exception type '{}' in throws",
                                parts.join(".")
                            ))
                            .with_file(sf.path.clone()),
                        );
                    }
                }
                if let Some(body) = &f.body {
                    let throws = throws_list_text(&f.sig);
                    walk_block(
                        body,
                        &throws,
                        &empty_caught,
                        &empty_exc_vars,
                        sf,
                        reporter,
                        current_pkg,
                        module_index,
                        rp,
                    );
                }
            }
            AS::Decl::Module(m) => {
                for f in &m.items {
                    for np in &f.sig.throws {
                        let parts: Vec<String> = np.path.iter().map(|i| i.0.clone()).collect();
                        if parts.len() == 1 && parts[0] == "Error" {
                            continue;
                        }
                        if lookup_symbol_kind(rp, current_pkg, &parts).is_none() {
                            reporter.emit(
                                Diagnostic::error(format!(
                                    "unknown exception type '{}' in throws",
                                    parts.join(".")
                                ))
                                .with_file(sf.path.clone()),
                            );
                        }
                    }
                    if let Some(body) = &f.body {
                        let throws = throws_list_text(&f.sig);
                        walk_block(
                            body,
                            &throws,
                            &empty_caught,
                            &empty_exc_vars,
                            sf,
                            reporter,
                            current_pkg,
                            module_index,
                            rp,
                        );
                    }
                }
            }
            _ => {}
        }
    }
}

/// Validate FFI declarations at the boundary level.
/// This includes:
/// 1. Extern function parameter types must be FFI-safe
/// 2. Extern function return types must be FFI-safe
/// 3. Extern functions cannot throw exceptions (C code cannot handle Arth exceptions)
fn validate_ffi_declarations(
    _current_pkg: &str,
    ast: &AS::FileAst,
    reporter: &mut Reporter,
    sf: &SourceFile,
) {
    for d in &ast.decls {
        if let AS::Decl::ExternFunc(ef) = d {
            // Validate all parameter types are FFI-safe
            for p in &ef.params {
                if !is_ffi_safe_namepath(&p.ty) {
                    let ty_name: String =
                        p.ty.path
                            .iter()
                            .map(|i| i.0.clone())
                            .collect::<Vec<_>>()
                            .join(".");
                    reporter.emit(
                        Diagnostic::error(format!(
                            "extern function '{}' parameter '{}' has non-FFI-safe type '{}'; \
                             only primitive types (int, float, bool, char, void) can cross the C boundary",
                            ef.name.0, p.name.0, ty_name
                        ))
                        .with_file(sf.path.clone()),
                    );
                }
            }

            // Validate return type is FFI-safe
            if let Some(ret_ty) = &ef.ret {
                if !is_ffi_safe_namepath(ret_ty) {
                    let ty_name: String = ret_ty
                        .path
                        .iter()
                        .map(|i| i.0.clone())
                        .collect::<Vec<_>>()
                        .join(".");
                    reporter.emit(
                        Diagnostic::error(format!(
                            "extern function '{}' has non-FFI-safe return type '{}'; \
                             only primitive types (int, float, bool, char, void) can cross the C boundary",
                            ef.name.0, ty_name
                        ))
                        .with_file(sf.path.clone()),
                    );
                }
            }

            // Validate FFI ownership attribute semantics
            let ownership = parse_ffi_ownership_attrs(&ef.attrs, sf, reporter);

            // @ffi_owned or @ffi_borrowed on void return is suspicious
            if ef.ret.is_none() {
                match ownership {
                    FfiOwnership::Owned => {
                        reporter.emit(
                            Diagnostic::warning(format!(
                                "extern function '{}' has @ffi_owned but returns void; \
                                 ownership semantics only apply to non-void return types",
                                ef.name.0
                            ))
                            .with_file(sf.path.clone()),
                        );
                    }
                    FfiOwnership::Borrowed => {
                        reporter.emit(
                            Diagnostic::warning(format!(
                                "extern function '{}' has @ffi_borrowed but returns void; \
                                 borrow semantics only apply to non-void return types",
                                ef.name.0
                            ))
                            .with_file(sf.path.clone()),
                        );
                    }
                    _ => {}
                }
            }

            // @ffi_transfers on function with no parameters is suspicious
            if ef.params.is_empty() && matches!(ownership, FfiOwnership::Transfers) {
                reporter.emit(
                    Diagnostic::warning(format!(
                        "extern function '{}' has @ffi_transfers but takes no parameters; \
                         transfer semantics apply to parameters passed to C",
                        ef.name.0
                    ))
                    .with_file(sf.path.clone()),
                );
            }
        }
    }
}

/// Parse FFI ownership attributes from an extern function declaration.
/// Validates mutual exclusivity and returns the appropriate FfiOwnership enum.
fn parse_ffi_ownership_attrs(
    attrs: &[AS::Attr],
    sf: &SourceFile,
    reporter: &mut Reporter,
) -> FfiOwnership {
    let mut found: Vec<(&str, FfiOwnership)> = Vec::new();

    for attr in attrs {
        let name: String = attr
            .name
            .path
            .iter()
            .map(|i| i.0.clone())
            .collect::<Vec<_>>()
            .join(".");
        match name.as_str() {
            "ffi_owned" => found.push(("ffi_owned", FfiOwnership::Owned)),
            "ffi_borrowed" => found.push(("ffi_borrowed", FfiOwnership::Borrowed)),
            "ffi_transfers" => found.push(("ffi_transfers", FfiOwnership::Transfers)),
            _ => {}
        }
    }

    match found.len() {
        0 => FfiOwnership::None,
        1 => found[0].1,
        _ => {
            // Multiple FFI ownership attributes - emit error
            let names: Vec<&str> = found.iter().map(|(n, _)| *n).collect();
            reporter.emit(
                Diagnostic::error(format!(
                    "extern function has multiple conflicting FFI ownership attributes: @{}; \
                     only one of @ffi_owned, @ffi_borrowed, or @ffi_transfers is allowed",
                    names.join(", @")
                ))
                .with_file(sf.path.clone()),
            );
            // Return the first one for continued analysis
            found[0].1
        }
    }
}

/// Check if an FFI attribute indicates ownership transfer semantics.
/// Returns Some((attr_name, is_transfer_to_c)) if found.
#[allow(dead_code)]
fn get_ffi_ownership_attr(attrs: &[AS::Attr]) -> Option<(&str, bool)> {
    for attr in attrs {
        let name: String = attr
            .name
            .path
            .iter()
            .map(|i| i.0.clone())
            .collect::<Vec<_>>()
            .join(".");
        match name.as_str() {
            // @ffi_owned - Arth takes ownership of the returned value (must cleanup)
            "ffi_owned" => return Some(("ffi_owned", false)),
            // @ffi_borrowed - Pointer is borrowed, read-only access
            "ffi_borrowed" => return Some(("ffi_borrowed", false)),
            // @ffi_transfers - Arth transfers ownership to C (moved, no cleanup)
            "ffi_transfers" => return Some(("ffi_transfers", true)),
            _ => {}
        }
    }
    None
}

#[allow(clippy::type_complexity)]
#[allow(clippy::type_complexity)]
fn collect_interface_methods_recursive(
    rp: &ResolvedProgram,
    ifc_index: &std::collections::HashMap<
        (String, String),
        (Vec<String>, Vec<AS::InterfaceMethod>, Vec<AS::NamePath>),
    >,
    pkg: &str,
    name: &str,
    out: &mut std::collections::HashMap<(String, Vec<String>, Option<String>), AS::InterfaceMethod>,
    // collect all generic param names across the visited interfaces (for substitution)
    generic_names: &mut std::collections::HashSet<String>,
    visiting: &mut std::collections::HashSet<(String, String)>,
) {
    let key = (pkg.to_string(), name.to_string());
    if !visiting.insert(key.clone()) {
        return;
    }
    if let Some((gens, methods, extends)) = ifc_index.get(&key) {
        // Record generics on this interface for later substitution
        for g in gens {
            generic_names.insert(g.clone());
        }
        // Add own methods (use sig for key)
        for m in methods {
            out.entry(sig_key(&m.sig)).or_insert_with(|| m.clone());
        }
        // Recurse into extends
        for ex in extends {
            let parts = np_to_vec(ex);
            if parts.is_empty() {
                continue;
            }
            let (epkg, ename) = if parts.len() == 1 {
                (pkg.to_string(), parts[0].clone())
            } else {
                (
                    parts[..parts.len() - 1].join("."),
                    parts.last().unwrap().clone(),
                )
            };
            // Only traverse if symbol kind is interface
            if let Some(ResolvedKind::Interface) = lookup_symbol_kind(rp, &epkg, &parts) {
                collect_interface_methods_recursive(
                    rp,
                    ifc_index,
                    &epkg,
                    &ename,
                    out,
                    generic_names,
                    visiting,
                );
            }
        }
    }
    visiting.remove(&(pkg.to_string(), name.to_string()));
}

#[allow(clippy::type_complexity)]
fn check_module_implements(
    current_pkg: &str,
    ast: &AS::FileAst,
    rp: &ResolvedProgram,
    reporter: &mut Reporter,
    sf: &SourceFile,
    ifc_index: &std::collections::HashMap<
        (String, String),
        (Vec<String>, Vec<AS::InterfaceMethod>, Vec<AS::NamePath>),
    >,
) {
    use std::collections::{HashMap, HashSet};
    // Build quick lookup for module methods
    let mut module_methods: HashMap<
        String,
        HashMap<(String, Vec<String>, Option<String>), AS::FuncSig>,
    > = HashMap::new();
    for d in &ast.decls {
        if let AS::Decl::Module(m) = d {
            let mut map: HashMap<(String, Vec<String>, Option<String>), AS::FuncSig> =
                HashMap::new();
            for f in &m.items {
                map.insert(sig_key(&f.sig), f.sig.clone());
            }
            module_methods.insert(m.name.0.clone(), map);
        }
    }

    for d in &ast.decls {
        let AS::Decl::Module(m) = d else { continue };
        for imp in &m.implements {
            let parts = np_to_vec(imp);
            let (epkg, ename) = if parts.len() == 1 {
                (current_pkg.to_string(), parts[0].clone())
            } else {
                (
                    parts[..parts.len() - 1].join("."),
                    parts.last().unwrap().clone(),
                )
            };
            match lookup_symbol_kind(rp, &epkg, &parts) {
                Some(ResolvedKind::Interface) => {
                    // Collect required methods and all generic param names in the interface hierarchy
                    let mut required: HashMap<
                        (String, Vec<String>, Option<String>),
                        AS::InterfaceMethod,
                    > = HashMap::new();
                    let mut visiting: HashSet<(String, String)> = HashSet::new();
                    let mut generic_names: HashSet<String> = HashSet::new();
                    collect_interface_methods_recursive(
                        rp,
                        ifc_index,
                        &epkg,
                        &ename,
                        &mut required,
                        &mut generic_names,
                        &mut visiting,
                    );

                    // Infer target type from module's method signatures
                    // Strategy: prefer non-primitive first params (struct types),
                    // but fall back to any first param (for modules on primitives like String)
                    let is_primitive = |name: &str| {
                        matches!(
                            name.to_ascii_lowercase().as_str(),
                            "int"
                                | "i8"
                                | "i16"
                                | "i32"
                                | "i64"
                                | "float"
                                | "f32"
                                | "f64"
                                | "bool"
                                | "char"
                                | "string"
                                | "void"
                                | "bytes"
                        )
                    };
                    // First try to find a non-primitive first param (struct/custom type)
                    let non_primitive_target: Option<String> = m
                        .items
                        .iter()
                        .filter_map(|f| f.sig.params.first())
                        .find_map(|p| {
                            let name = p.ty.path.last().map(|i| i.0.clone())?;
                            if is_primitive(&name) {
                                None
                            } else {
                                Some(name)
                            }
                        });
                    // Fall back to any first param (handles String modules, etc.)
                    let any_target: Option<String> = m
                        .items
                        .iter()
                        .find_map(|f| f.sig.params.first())
                        .and_then(|p| p.ty.path.last().map(|i| i.0.clone()));
                    let inferred_target = non_primitive_target.or(any_target);

                    // Build generic param substitution map from type arguments in implements clause
                    // e.g., for `implements Container<int>`, map T -> int
                    let mut generic_subst: HashMap<String, String> = HashMap::new();
                    if let Some((gen_names, _, _)) = ifc_index.get(&(epkg.clone(), ename.clone())) {
                        for (i, gen_name) in gen_names.iter().enumerate() {
                            if let Some(type_arg) = imp.type_args.get(i) {
                                // Convert type arg NamePath to string
                                let type_arg_str = type_arg
                                    .path
                                    .iter()
                                    .map(|id| id.0.as_str())
                                    .collect::<Vec<_>>()
                                    .join(".");
                                generic_subst.insert(gen_name.clone(), type_arg_str);
                            }
                        }
                    }

                    // Build required methods with full signature info (including throws)
                    // Interface methods don't have 'self', but module methods have explicit receiver
                    // So we prepend the inferred target type to interface method params
                    // Skip default methods - they are not required to be implemented
                    struct RequiredMethod {
                        name: String,
                        params: Vec<String>,
                        ret: Option<String>,
                        throws: Vec<String>,
                    }
                    let mut required_methods: Vec<RequiredMethod> = Vec::new();

                    for (key, method) in required.into_iter() {
                        // Default methods don't need to be implemented
                        if method.default_body.is_some() {
                            continue;
                        }
                        let (n, params, ret) = key;
                        let sub = |s: &str| -> String {
                            // First check explicit type arguments from implements clause
                            if let Some(subst) = generic_subst.get(s) {
                                subst.clone()
                            // Then fall back to inferred target for unspecified generics
                            } else if generic_names.contains(s) {
                                inferred_target.clone().unwrap_or_else(|| s.to_string())
                            } else {
                                s.to_string()
                            }
                        };
                        // Substitute generic params in interface signature
                        let substituted_params: Vec<String> =
                            params.iter().map(|p| sub(p)).collect();
                        let new_ret = ret.as_ref().map(|r| sub(r));
                        // Inject receiver type as first parameter
                        let mut new_params = Vec::new();
                        if let Some(ref target) = inferred_target {
                            new_params.push(target.clone());
                        }
                        new_params.extend(substituted_params);
                        // Substitute generics in throws clause
                        let throws: Vec<String> = method
                            .sig
                            .throws
                            .iter()
                            .map(|t| sub(&type_text(t)))
                            .collect();
                        required_methods.push(RequiredMethod {
                            name: n,
                            params: new_params,
                            ret: new_ret,
                            throws,
                        });
                    }

                    // Validate presence and signature compatibility in module
                    let mm = module_methods.get(&m.name.0).cloned().unwrap_or_default();
                    for req in required_methods {
                        let key = (req.name.clone(), req.params.clone(), req.ret.clone());
                        if let Some(impl_sig) = mm.get(&key) {
                            // Method found - check throws compatibility
                            // Implementation can throw fewer exceptions, not more
                            let impl_throws: HashSet<String> =
                                impl_sig.throws.iter().map(type_text).collect();
                            let iface_throws: HashSet<String> =
                                req.throws.iter().cloned().collect();

                            // Check that impl doesn't throw anything not in interface
                            for impl_ex in &impl_throws {
                                if !iface_throws.contains(impl_ex) {
                                    reporter.emit(
                                        Diagnostic::error(format!(
                                            "method '{}' in module '{}' throws '{}' which is not declared in interface '{}'. \
                                            Implementation cannot throw additional exceptions.",
                                            req.name,
                                            m.name.0,
                                            impl_ex,
                                            parts.join(".")
                                        ))
                                        .with_file(sf.path.clone()),
                                    );
                                }
                            }
                        } else {
                            // Method not found - provide helpful error with expected signature
                            let expected_sig = format!(
                                "{}({}){}{}",
                                req.name,
                                req.params.join(", "),
                                req.ret
                                    .as_ref()
                                    .map(|r| format!(" -> {}", r))
                                    .unwrap_or_default(),
                                if req.throws.is_empty() {
                                    String::new()
                                } else {
                                    format!(" throws ({})", req.throws.join(", "))
                                }
                            );
                            reporter.emit(
                                Diagnostic::error(format!(
                                    "module '{}' does not implement method '{}' required by interface '{}'. \
                                    Expected signature: {}",
                                    m.name.0,
                                    req.name,
                                    parts.join("."),
                                    expected_sig
                                ))
                                .with_file(sf.path.clone()),
                            );
                        }
                    }
                }
                Some(_) => reporter.emit(
                    Diagnostic::error(format!(
                        "module '{}' implements non-interface '{}'",
                        m.name.0,
                        parts.join(".")
                    ))
                    .with_file(sf.path.clone()),
                ),
                None => reporter.emit(
                    Diagnostic::error(format!(
                        "module '{}' implements unknown '{}'",
                        m.name.0,
                        parts.join(".")
                    ))
                    .with_file(sf.path.clone()),
                ),
            }
        }
    }
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_defassign_loops_switch_try;
#[cfg(test)]
mod tests_phase2;

/// Seed module function signatures from the StdlibIndex.
/// This replaces the manual seed_stdlib_signatures() with actual parsed signatures from .arth files.
fn seed_stdlib_from_index(
    stdlib: &StdlibIndex,
    module: &mut std::collections::HashMap<(String, String, String), Vec<AS::FuncSig>>,
) {
    for ((pkg, mod_name, func_name), sig) in stdlib.all_module_functions() {
        module
            .entry((pkg, mod_name, func_name))
            .or_default()
            .push(sig.clone());
    }
}

/// Build a map of interface implementations across all modules.
fn build_implements_index(files: &[(SourceFile, AS::FileAst)]) -> ImplementsIndex {
    // Infer target type from module name using naming convention.
    // E.g., "PointFns" -> "Point", "StringOps" -> "String"
    fn infer_target_type_from_module_name(module_name: &str) -> Option<String> {
        for suffix in &["Fns", "Ops", "Impl", "Module"] {
            if module_name.ends_with(suffix) && module_name.len() > suffix.len() {
                return Some(module_name[..module_name.len() - suffix.len()].to_string());
            }
        }
        None
    }

    let mut index = ImplementsIndex::new();

    for (_sf, ast) in files {
        let pkg = match pkg_string(ast) {
            Some(p) => p,
            None => continue,
        };

        for d in &ast.decls {
            if let Decl::Module(m) = d {
                if m.implements.is_empty() {
                    continue;
                }

                let is_primitive = |name: &str| {
                    matches!(
                        name.to_ascii_lowercase().as_str(),
                        "int"
                            | "i8"
                            | "i16"
                            | "i32"
                            | "i64"
                            | "float"
                            | "f32"
                            | "f64"
                            | "bool"
                            | "char"
                            | "string"
                            | "void"
                            | "bytes"
                    )
                };

                // Strategy 1: Prefer non-primitive first parameters
                let non_primitive_target: Option<String> = m
                    .items
                    .iter()
                    .filter_map(|f| f.sig.params.first())
                    .find_map(|p| {
                        let name = p.ty.path.last().map(|i| i.0.clone())?;
                        if is_primitive(&name) {
                            None
                        } else {
                            Some(name)
                        }
                    });

                // Strategy 2: Fall back to any first parameter
                let any_target: Option<String> = m
                    .items
                    .iter()
                    .find_map(|f| f.sig.params.first())
                    .and_then(|p| p.ty.path.last().map(|i| i.0.clone()));

                // Strategy 3: Naming convention
                let name_target = infer_target_type_from_module_name(&m.name.0);

                if let Some(name) = non_primitive_target.or(any_target).or(name_target) {
                    let key = (pkg.clone(), name);
                    let implemented = index.entry(key).or_default();

                    for imp in &m.implements {
                        let ity = map_name_to_ty(imp);
                        implemented.insert(ity);
                    }
                }
            }
        }
    }
    index
}

/// Check if a type satisfies an interface bound.
fn satisfies_bound(ty: &Ty, bound: &AS::NamePath, env: &Env) -> bool {
    let bound_ty = env.map_name_to_ty_with_params(bound);
    if same_type(ty, &bound_ty) {
        return true;
    }

    match ty {
        Ty::Named(path) | Ty::Generic { path, .. } => {
            if path.is_empty() {
                return false;
            }
            let type_name = path.last().unwrap().clone();
            let pkg = if path.len() > 1 {
                path[..path.len() - 1].join(".")
            } else {
                env.current_pkg.clone().unwrap_or_default()
            };

            if let Some(implemented) = env.implements.get(&(pkg, type_name)) {
                for imp in implemented {
                    if same_type(imp, &bound_ty) {
                        return true;
                    }
                }
            }
            false
        }
        _ => {
            // Primitives don't implement user interfaces (unless explicitly added to index via built-in modules)
            false
        }
    }
}
