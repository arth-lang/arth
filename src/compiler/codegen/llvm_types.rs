//! LLVM Type Layout System for Arth Native Compilation
//!
//! This module computes struct and enum layouts for native LLVM IR emission.
//! It handles field alignment, size calculation, and generates proper LLVM
//! type declarations.
//!
//! # Design Notes
//!
//! - Uses standard C ABI alignment rules (System V AMD64 ABI for x86_64)
//! - All Arth types map to primitive LLVM types or struct types
//! - Enums use tagged union representation: { i32 tag, payload... }
//! - Strings are represented as { ptr, i64 } (pointer + length)

use std::collections::HashMap;

use crate::compiler::ir::Ty;

// =============================================================================
// Field and Struct Layout
// =============================================================================

/// Layout information for a single struct field.
#[derive(Clone, Debug)]
pub struct FieldLayout {
    /// Field name from source code.
    pub name: String,
    /// LLVM type for this field.
    pub llvm_ty: String,
    /// Byte offset from start of struct.
    pub byte_offset: u32,
    /// Index in LLVM struct type (0-based).
    pub llvm_index: u32,
    /// Size in bytes.
    pub size: u32,
    /// Alignment in bytes.
    pub align: u32,
}

/// Complete layout for a struct type.
#[derive(Clone, Debug)]
pub struct StructLayout {
    /// Struct name (fully qualified).
    pub name: String,
    /// LLVM type name (e.g., "%MyStruct").
    pub llvm_type_name: String,
    /// LLVM type definition (e.g., "type { i64, ptr, double }").
    pub llvm_type_def: String,
    /// Field layouts in declaration order.
    pub fields: Vec<FieldLayout>,
    /// Total size in bytes (including padding).
    pub size: u32,
    /// Alignment requirement in bytes.
    pub alignment: u32,
}

/// Represents an Arth type for layout computation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ArthType {
    /// 64-bit signed integer (i64).
    Int,
    /// 64-bit floating point (double).
    Float,
    /// Boolean (i1, but stored as i8 for ABI compatibility).
    Bool,
    /// String (represented as { ptr, i64 } struct).
    String,
    /// Pointer to another type.
    Ptr,
    /// Reference to a named struct type.
    Struct(String),
    /// Reference to a named enum type.
    Enum(String),
    /// Optional<T> - represented as { i1 is_some, T value }.
    Optional(Box<ArthType>),
    /// Void (no value).
    Void,
}

/// Field definition for struct layout computation.
#[derive(Clone, Debug)]
pub struct FieldDef {
    pub name: String,
    pub ty: ArthType,
}

/// Struct definition for layout computation.
#[derive(Clone, Debug)]
pub struct StructDef {
    pub name: String,
    pub fields: Vec<FieldDef>,
}

// =============================================================================
// Enum Layout
// =============================================================================

/// Layout for an enum variant.
#[derive(Clone, Debug)]
pub struct VariantLayout {
    /// Variant name.
    pub name: String,
    /// Tag value (discriminant).
    pub tag: i32,
    /// Payload types (empty for unit variants).
    pub payload_types: Vec<ArthType>,
    /// Combined payload size in bytes.
    pub payload_size: u32,
}

/// Complete layout for an enum type.
#[derive(Clone, Debug)]
pub struct EnumLayout {
    /// Enum name (fully qualified).
    pub name: String,
    /// LLVM type name (e.g., "%MyEnum").
    pub llvm_type_name: String,
    /// LLVM type definition.
    pub llvm_type_def: String,
    /// Variant layouts.
    pub variants: Vec<VariantLayout>,
    /// Total size in bytes (tag + max payload + padding).
    pub size: u32,
    /// Alignment requirement.
    pub alignment: u32,
    /// Size of the tag field (typically 4 bytes for i32).
    pub tag_size: u32,
    /// Maximum payload size across all variants.
    pub max_payload_size: u32,
}

/// Enum variant definition.
#[derive(Clone, Debug)]
pub struct VariantDef {
    pub name: String,
    pub payload_types: Vec<ArthType>,
}

/// Enum definition for layout computation.
#[derive(Clone, Debug)]
pub struct EnumDef {
    pub name: String,
    pub variants: Vec<VariantDef>,
}

// =============================================================================
// Type Registry
// =============================================================================

/// Registry of all struct and enum types for a module.
#[derive(Clone, Debug, Default)]
pub struct TypeRegistry {
    pub structs: HashMap<String, StructLayout>,
    pub enums: HashMap<String, EnumLayout>,
}

impl TypeRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a struct definition and compute its layout.
    pub fn register_struct(&mut self, def: StructDef) {
        let layout = compute_struct_layout(&def, self);
        self.structs.insert(def.name.clone(), layout);
    }

    /// Register an enum definition and compute its layout.
    pub fn register_enum(&mut self, def: EnumDef) {
        let layout = compute_enum_layout(&def, self);
        self.enums.insert(def.name.clone(), layout);
    }

    /// Get struct layout by name.
    pub fn get_struct(&self, name: &str) -> Option<&StructLayout> {
        self.structs.get(name)
    }

    /// Get enum layout by name.
    pub fn get_enum(&self, name: &str) -> Option<&EnumLayout> {
        self.enums.get(name)
    }

    /// Emit all LLVM type definitions.
    pub fn emit_type_definitions(&self) -> String {
        let mut buf = String::new();

        // Emit struct types
        for layout in self.structs.values() {
            buf.push_str(&format!(
                "{} = {}\n",
                layout.llvm_type_name, layout.llvm_type_def
            ));
        }

        // Emit enum types
        for layout in self.enums.values() {
            buf.push_str(&format!(
                "{} = {}\n",
                layout.llvm_type_name, layout.llvm_type_def
            ));
        }

        buf
    }
}

// =============================================================================
// Layout Computation
// =============================================================================

/// Get the size and alignment of an ArthType.
fn type_size_align(ty: &ArthType, registry: &TypeRegistry) -> (u32, u32) {
    match ty {
        ArthType::Int => (8, 8),     // i64
        ArthType::Float => (8, 8),   // double
        ArthType::Bool => (1, 1),    // i8 for ABI compatibility
        ArthType::Ptr => (8, 8),     // pointer on 64-bit
        ArthType::String => (16, 8), // { ptr, i64 } = 16 bytes, 8-byte aligned
        ArthType::Void => (0, 1),
        ArthType::Struct(name) => {
            if let Some(layout) = registry.get_struct(name) {
                (layout.size, layout.alignment)
            } else {
                // Unknown struct - treat as opaque pointer
                (8, 8)
            }
        }
        ArthType::Enum(name) => {
            if let Some(layout) = registry.get_enum(name) {
                (layout.size, layout.alignment)
            } else {
                // Unknown enum - treat as i64
                (8, 8)
            }
        }
        ArthType::Optional(inner) => {
            let (inner_size, inner_align) = type_size_align(inner, registry);
            // { i1 is_some, T value } - i1 is stored as i8 for ABI
            let total_size = align_up(1, inner_align) + inner_size;
            let total_size = align_up(total_size, inner_align);
            (total_size, inner_align.max(1))
        }
    }
}

/// Get the LLVM type string for an ArthType.
fn type_to_llvm(ty: &ArthType, _registry: &TypeRegistry) -> String {
    match ty {
        ArthType::Int => "i64".to_string(),
        ArthType::Float => "double".to_string(),
        ArthType::Bool => "i8".to_string(), // i8 for ABI compatibility
        ArthType::Ptr => "ptr".to_string(),
        ArthType::String => "{ ptr, i64 }".to_string(),
        ArthType::Void => "void".to_string(),
        ArthType::Struct(name) => format!("%{}", sanitize_llvm_name(name)),
        ArthType::Enum(name) => format!("%{}", sanitize_llvm_name(name)),
        ArthType::Optional(inner) => {
            let inner_ty = type_to_llvm(inner, _registry);
            format!("{{ i8, {} }}", inner_ty)
        }
    }
}

/// Sanitize a name for use as an LLVM identifier.
fn sanitize_llvm_name(name: &str) -> String {
    name.replace(['.', ':'], "_")
}

/// Align a value up to the given alignment.
fn align_up(value: u32, align: u32) -> u32 {
    if align == 0 {
        return value;
    }
    value.div_ceil(align) * align
}

/// Compute the layout for a struct.
pub fn compute_struct_layout(def: &StructDef, registry: &TypeRegistry) -> StructLayout {
    let mut fields = Vec::new();
    let mut offset: u32 = 0;
    let mut max_align: u32 = 1;
    let mut llvm_types = Vec::new();

    for (idx, field) in def.fields.iter().enumerate() {
        let (size, align) = type_size_align(&field.ty, registry);
        let llvm_ty = type_to_llvm(&field.ty, registry);

        // Update max alignment
        max_align = max_align.max(align);

        // Align offset for this field
        offset = align_up(offset, align);

        fields.push(FieldLayout {
            name: field.name.clone(),
            llvm_ty: llvm_ty.clone(),
            byte_offset: offset,
            llvm_index: idx as u32,
            size,
            align,
        });

        llvm_types.push(llvm_ty);
        offset += size;
    }

    // Final struct size includes tail padding for alignment
    let total_size = align_up(offset, max_align);

    // Generate LLVM type definition
    let llvm_type_name = format!("%{}", sanitize_llvm_name(&def.name));
    let llvm_type_def = if llvm_types.is_empty() {
        "type {}".to_string()
    } else {
        format!("type {{ {} }}", llvm_types.join(", "))
    };

    StructLayout {
        name: def.name.clone(),
        llvm_type_name,
        llvm_type_def,
        fields,
        size: total_size,
        alignment: max_align,
    }
}

/// Compute the layout for an enum (tagged union).
pub fn compute_enum_layout(def: &EnumDef, registry: &TypeRegistry) -> EnumLayout {
    let tag_size: u32 = 4; // i32 for tag
    let tag_align: u32 = 4;
    let mut max_payload_size: u32 = 0;
    let mut max_payload_align: u32 = 1;
    let mut variants = Vec::new();

    for (tag, variant) in def.variants.iter().enumerate() {
        let mut payload_size: u32 = 0;
        let mut payload_align: u32 = 1;

        for ty in &variant.payload_types {
            let (size, align) = type_size_align(ty, registry);
            payload_align = payload_align.max(align);
            payload_size = align_up(payload_size, align) + size;
        }

        max_payload_size = max_payload_size.max(payload_size);
        max_payload_align = max_payload_align.max(payload_align);

        variants.push(VariantLayout {
            name: variant.name.clone(),
            tag: tag as i32,
            payload_types: variant.payload_types.clone(),
            payload_size,
        });
    }

    // Enum layout: { i32 tag, [max_payload_size x i8] }
    let alignment = tag_align.max(max_payload_align);
    let payload_offset = align_up(tag_size, max_payload_align);
    let total_size = align_up(payload_offset + max_payload_size, alignment);

    let llvm_type_name = format!("%{}", sanitize_llvm_name(&def.name));

    // Use a byte array for the payload to allow variant-specific casting
    let llvm_type_def = if max_payload_size > 0 {
        format!("type {{ i32, [{} x i8] }}", max_payload_size)
    } else {
        "type { i32 }".to_string()
    };

    EnumLayout {
        name: def.name.clone(),
        llvm_type_name,
        llvm_type_def,
        variants,
        size: total_size,
        alignment,
        tag_size,
        max_payload_size,
    }
}

// =============================================================================
// Conversion from IR types
// =============================================================================

impl From<&Ty> for ArthType {
    fn from(ty: &Ty) -> Self {
        match ty {
            Ty::I64 => ArthType::Int,
            Ty::F64 => ArthType::Float,
            Ty::I1 => ArthType::Bool,
            Ty::Ptr => ArthType::Ptr,
            Ty::Void => ArthType::Void,
            Ty::Struct(name) => ArthType::Struct(name.clone()),
            Ty::Enum(name) => ArthType::Enum(name.clone()),
            Ty::Optional(inner) => ArthType::Optional(Box::new(ArthType::from(inner.as_ref()))),
            Ty::String => ArthType::String,
        }
    }
}

// =============================================================================
// GEP (GetElementPtr) Emission Helpers
// =============================================================================

impl StructLayout {
    /// Generate LLVM GEP instruction for field access.
    ///
    /// Returns: (gep_instruction, result_type)
    /// Example: ("%ptr = getelementptr inbounds %MyStruct, ptr %obj, i32 0, i32 2", "i64")
    pub fn emit_field_gep(
        &self,
        result_reg: u32,
        obj_reg: u32,
        field_name: &str,
    ) -> Option<(String, String)> {
        let field = self.fields.iter().find(|f| f.name == field_name)?;

        let gep = format!(
            "%{} = getelementptr inbounds {}, ptr %{}, i32 0, i32 {}",
            result_reg, self.llvm_type_name, obj_reg, field.llvm_index
        );

        Some((gep, field.llvm_ty.clone()))
    }

    /// Generate LLVM GEP instruction for field access by index.
    pub fn emit_field_gep_by_index(
        &self,
        result_reg: u32,
        obj_reg: u32,
        field_index: usize,
    ) -> Option<(String, String)> {
        let field = self.fields.get(field_index)?;

        let gep = format!(
            "%{} = getelementptr inbounds {}, ptr %{}, i32 0, i32 {}",
            result_reg, self.llvm_type_name, obj_reg, field.llvm_index
        );

        Some((gep, field.llvm_ty.clone()))
    }

    /// Generate LLVM alloca for this struct type.
    pub fn emit_alloca(&self, result_reg: u32) -> String {
        format!("%{} = alloca {}", result_reg, self.llvm_type_name)
    }
}

impl EnumLayout {
    /// Generate LLVM code to extract the tag from an enum value.
    ///
    /// Returns the GEP + load instructions.
    pub fn emit_tag_extract(&self, result_reg: u32, enum_ptr_reg: u32) -> String {
        format!(
            "%tag_ptr_{0} = getelementptr inbounds {1}, ptr %{2}, i32 0, i32 0\n  \
             %{0} = load i32, ptr %tag_ptr_{0}",
            result_reg, self.llvm_type_name, enum_ptr_reg
        )
    }

    /// Generate LLVM code to get a pointer to the payload area.
    pub fn emit_payload_ptr(&self, result_reg: u32, enum_ptr_reg: u32) -> String {
        if self.max_payload_size > 0 {
            format!(
                "%{} = getelementptr inbounds {}, ptr %{}, i32 0, i32 1",
                result_reg, self.llvm_type_name, enum_ptr_reg
            )
        } else {
            // No payload - return null or the enum ptr itself
            format!("%{} = bitcast ptr %{} to ptr", result_reg, enum_ptr_reg)
        }
    }

    /// Get the tag value for a variant by name.
    pub fn get_variant_tag(&self, variant_name: &str) -> Option<i32> {
        self.variants
            .iter()
            .find(|v| v.name == variant_name)
            .map(|v| v.tag)
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_struct_layout() {
        let registry = TypeRegistry::new();

        let def = StructDef {
            name: "Point".to_string(),
            fields: vec![
                FieldDef {
                    name: "x".to_string(),
                    ty: ArthType::Int,
                },
                FieldDef {
                    name: "y".to_string(),
                    ty: ArthType::Int,
                },
            ],
        };

        let layout = compute_struct_layout(&def, &registry);

        assert_eq!(layout.name, "Point");
        assert_eq!(layout.llvm_type_name, "%Point");
        assert_eq!(layout.llvm_type_def, "type { i64, i64 }");
        assert_eq!(layout.size, 16);
        assert_eq!(layout.alignment, 8);
        assert_eq!(layout.fields.len(), 2);
        assert_eq!(layout.fields[0].byte_offset, 0);
        assert_eq!(layout.fields[1].byte_offset, 8);
    }

    #[test]
    fn test_mixed_type_struct_layout() {
        let registry = TypeRegistry::new();

        let def = StructDef {
            name: "Mixed".to_string(),
            fields: vec![
                FieldDef {
                    name: "flag".to_string(),
                    ty: ArthType::Bool,
                },
                FieldDef {
                    name: "count".to_string(),
                    ty: ArthType::Int,
                },
                FieldDef {
                    name: "value".to_string(),
                    ty: ArthType::Float,
                },
            ],
        };

        let layout = compute_struct_layout(&def, &registry);

        assert_eq!(layout.llvm_type_def, "type { i8, i64, double }");
        // bool at 0, then padding to 8, int at 8, double at 16
        assert_eq!(layout.fields[0].byte_offset, 0);
        assert_eq!(layout.fields[1].byte_offset, 8);
        assert_eq!(layout.fields[2].byte_offset, 16);
        assert_eq!(layout.size, 24);
    }

    #[test]
    fn test_simple_enum_layout() {
        let registry = TypeRegistry::new();

        let def = EnumDef {
            name: "Option".to_string(),
            variants: vec![
                VariantDef {
                    name: "None".to_string(),
                    payload_types: vec![],
                },
                VariantDef {
                    name: "Some".to_string(),
                    payload_types: vec![ArthType::Int],
                },
            ],
        };

        let layout = compute_enum_layout(&def, &registry);

        assert_eq!(layout.name, "Option");
        assert_eq!(layout.llvm_type_name, "%Option");
        assert_eq!(layout.tag_size, 4);
        assert_eq!(layout.max_payload_size, 8);
        assert_eq!(layout.variants.len(), 2);
        assert_eq!(layout.variants[0].tag, 0);
        assert_eq!(layout.variants[1].tag, 1);
    }

    #[test]
    fn test_gep_emission() {
        let registry = TypeRegistry::new();

        let def = StructDef {
            name: "Point".to_string(),
            fields: vec![
                FieldDef {
                    name: "x".to_string(),
                    ty: ArthType::Int,
                },
                FieldDef {
                    name: "y".to_string(),
                    ty: ArthType::Int,
                },
            ],
        };

        let layout = compute_struct_layout(&def, &registry);

        let (gep, ty) = layout.emit_field_gep(10, 5, "y").unwrap();
        assert_eq!(
            gep,
            "%10 = getelementptr inbounds %Point, ptr %5, i32 0, i32 1"
        );
        assert_eq!(ty, "i64");
    }

    #[test]
    fn test_enum_tag_extract() {
        let registry = TypeRegistry::new();

        let def = EnumDef {
            name: "Result".to_string(),
            variants: vec![
                VariantDef {
                    name: "Ok".to_string(),
                    payload_types: vec![ArthType::Int],
                },
                VariantDef {
                    name: "Err".to_string(),
                    payload_types: vec![ArthType::String],
                },
            ],
        };

        let layout = compute_enum_layout(&def, &registry);

        let code = layout.emit_tag_extract(10, 5);
        assert!(code.contains("getelementptr"));
        assert!(code.contains("load i32"));
    }

    #[test]
    fn test_nested_struct_layout() {
        let mut registry = TypeRegistry::new();

        // Register inner struct first
        let inner_def = StructDef {
            name: "Point".to_string(),
            fields: vec![
                FieldDef {
                    name: "x".to_string(),
                    ty: ArthType::Int,
                },
                FieldDef {
                    name: "y".to_string(),
                    ty: ArthType::Int,
                },
            ],
        };
        registry.register_struct(inner_def);

        // Register outer struct that references inner
        let outer_def = StructDef {
            name: "Line".to_string(),
            fields: vec![
                FieldDef {
                    name: "start".to_string(),
                    ty: ArthType::Struct("Point".to_string()),
                },
                FieldDef {
                    name: "end".to_string(),
                    ty: ArthType::Struct("Point".to_string()),
                },
            ],
        };
        let outer_layout = compute_struct_layout(&outer_def, &registry);

        // Line should contain two Points (16 bytes each)
        assert_eq!(outer_layout.size, 32);
        assert_eq!(outer_layout.alignment, 8);
        assert_eq!(outer_layout.fields[0].byte_offset, 0);
        assert_eq!(outer_layout.fields[1].byte_offset, 16);
    }

    #[test]
    fn test_optional_type_layout() {
        let registry = TypeRegistry::new();

        let def = StructDef {
            name: "MaybeInt".to_string(),
            fields: vec![FieldDef {
                name: "value".to_string(),
                ty: ArthType::Optional(Box::new(ArthType::Int)),
            }],
        };

        let layout = compute_struct_layout(&def, &registry);

        // Optional<Int> = { i8 is_some, i64 value } = 16 bytes (with padding)
        // The struct containing it has the same size
        assert_eq!(layout.llvm_type_def, "type { { i8, i64 } }");
        assert!(layout.size >= 9); // At least 1 + 8 bytes
    }

    #[test]
    fn test_string_type_layout() {
        let registry = TypeRegistry::new();

        let def = StructDef {
            name: "Person".to_string(),
            fields: vec![
                FieldDef {
                    name: "name".to_string(),
                    ty: ArthType::String,
                },
                FieldDef {
                    name: "age".to_string(),
                    ty: ArthType::Int,
                },
            ],
        };

        let layout = compute_struct_layout(&def, &registry);

        // String = { ptr, i64 } = 16 bytes, Int = 8 bytes
        assert_eq!(layout.size, 24);
        assert_eq!(layout.fields[0].byte_offset, 0);
        assert_eq!(layout.fields[0].llvm_ty, "{ ptr, i64 }");
        assert_eq!(layout.fields[1].byte_offset, 16);
    }

    #[test]
    fn test_enum_with_struct_payload() {
        let mut registry = TypeRegistry::new();

        // Register payload struct
        let payload_def = StructDef {
            name: "Error".to_string(),
            fields: vec![
                FieldDef {
                    name: "code".to_string(),
                    ty: ArthType::Int,
                },
                FieldDef {
                    name: "message".to_string(),
                    ty: ArthType::String,
                },
            ],
        };
        registry.register_struct(payload_def);

        // Register enum with struct payload
        let enum_def = EnumDef {
            name: "Result".to_string(),
            variants: vec![
                VariantDef {
                    name: "Ok".to_string(),
                    payload_types: vec![ArthType::Int],
                },
                VariantDef {
                    name: "Err".to_string(),
                    payload_types: vec![ArthType::Struct("Error".to_string())],
                },
            ],
        };

        let layout = compute_enum_layout(&enum_def, &registry);

        // Err payload (Error struct) is 24 bytes, larger than Ok (8 bytes)
        assert_eq!(layout.max_payload_size, 24);
        assert_eq!(layout.tag_size, 4);
    }

    #[test]
    fn test_ir_ty_conversion() {
        use crate::compiler::ir::Ty;

        // Test primitive types
        assert_eq!(ArthType::from(&Ty::I64), ArthType::Int);
        assert_eq!(ArthType::from(&Ty::F64), ArthType::Float);
        assert_eq!(ArthType::from(&Ty::I1), ArthType::Bool);
        assert_eq!(ArthType::from(&Ty::Ptr), ArthType::Ptr);
        assert_eq!(ArthType::from(&Ty::Void), ArthType::Void);

        // Test complex types
        assert_eq!(
            ArthType::from(&Ty::Struct("Point".to_string())),
            ArthType::Struct("Point".to_string())
        );
        assert_eq!(
            ArthType::from(&Ty::Enum("Result".to_string())),
            ArthType::Enum("Result".to_string())
        );
        assert_eq!(ArthType::from(&Ty::String), ArthType::String);

        // Test optional
        let opt_int = Ty::Optional(Box::new(Ty::I64));
        match ArthType::from(&opt_int) {
            ArthType::Optional(inner) => {
                assert_eq!(*inner, ArthType::Int);
            }
            _ => panic!("Expected Optional"),
        }
    }
}
