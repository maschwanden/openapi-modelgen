//! Model generation: parsing OpenAPI schemas into entities and writing the
//! `model.rs` / `validation.rs` / `default.rs` files of the generated crate.

pub(crate) mod parse;
pub(crate) mod write;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntityKind {
    /// From `components.schemas` — derives Serialize + Deserialize.
    Schema,
    /// From operation query parameters — derives only Deserialize.
    Query,
}

#[derive(Debug, PartialEq)]
pub struct Field {
    /// Field name as it appears in the struct (e.g. `name`, `limit`).
    pub name: String,
    /// Base Rust type before Option wrapping (e.g. `String`, `i64`, `Vec<Foo>`).
    pub rust_type: String,
    /// Whether the field is wrapped in `Option<T>` (not required or nullable).
    pub is_optional: bool,
    /// Validation constraints extracted from the OpenAPI schema.
    pub constraints: Constraints,
    /// Default value from the OpenAPI schema, if present and supported.
    pub default_value: Option<serde_json::Value>,
}

#[derive(Debug, PartialEq)]
pub enum Entity {
    /// A struct generated from an object schema or query parameters.
    Struct(StructDef),
    /// A standalone enum generated from a top-level string enum schema.
    Enum(EnumDef),
    /// A tagged/untagged union generated from a top-level `oneOf` schema.
    Union(UnionDef),
    /// A type alias generated from a schema whose root is an array,
    /// e.g. `pub type TagArray = Vec<Tag>;`.
    Alias(AliasDef),
}

/// A top-level type alias generated from an array schema.
#[derive(Debug, PartialEq)]
pub struct AliasDef {
    /// Alias name in PascalCase (e.g. `TagArray`).
    pub name: String,
    /// The aliased Rust type (e.g. `Vec<Tag>`).
    pub rust_type: String,
}

#[derive(Debug, PartialEq)]
pub struct StructDef {
    /// Struct name (e.g. `Foo`, `GetThingsQuery`).
    pub name: String,
    /// Whether this is a schema or query entity (affects derive macros).
    pub kind: EntityKind,
    /// Ordered list of fields.
    pub fields: Vec<Field>,
    /// String enums generated from inline enum fields.
    pub enums: Vec<EnumDef>,
}

#[derive(Debug, PartialEq)]
pub struct EnumDef {
    /// Enum name in PascalCase (e.g. `ChargingFrequency`).
    pub name: String,
    /// Variants in PascalCase with their original snake_case values.
    pub variants: Vec<(String, String)>,
}

/// A union type generated from a top-level `oneOf` schema.
///
/// When `tag` is `Some(prop)` (a `discriminator` was present), it maps to a
/// serde internally-tagged enum (`#[serde(tag = "prop")]`); otherwise it maps
/// to an untagged enum (`#[serde(untagged)]`).
#[derive(Debug, PartialEq)]
pub struct UnionDef {
    /// Enum name in PascalCase (e.g. `Pet`).
    pub name: String,
    /// One variant per `oneOf` member.
    pub variants: Vec<UnionVariant>,
    /// `Some(property_name)` for a discriminated (tagged) union, `None` for untagged.
    pub tag: Option<String>,
}

#[derive(Debug, PartialEq)]
pub struct UnionVariant {
    /// Variant name in PascalCase (e.g. `Dog`).
    pub variant_name: String,
    /// The referenced Rust type wrapped by this variant (e.g. `Dog`).
    pub inner_type: String,
    /// Wire value from a `discriminator.mapping` entry, if it renames the variant.
    pub wire_value: Option<String>,
}

/// Constraints extracted from a single field's OpenAPI schema.
#[derive(Debug, PartialEq)]
pub enum Constraints {
    /// No constraints — field needs no validation checks.
    None,
    String {
        min_length: Option<usize>,
        max_length: Option<usize>,
        pattern: Option<String>,
        enumeration: Vec<String>,
    },
    Integer {
        minimum: Option<i64>,
        maximum: Option<i64>,
        exclusive_minimum: bool,
        exclusive_maximum: bool,
        multiple_of: Option<i64>,
        enumeration: Vec<i64>,
    },
    Number {
        minimum: Option<f64>,
        maximum: Option<f64>,
        exclusive_minimum: bool,
        exclusive_maximum: bool,
        multiple_of: Option<f64>,
    },
    Array {
        min_items: Option<usize>,
        max_items: Option<usize>,
        unique_items: bool,
    },
    /// Field is a `$ref` to another generated struct — call `.validate()`.
    Nested,
    /// Field is `Vec<$ref>` — iterate and call `.validate()` on each element.
    VecNested,
}

impl Constraints {
    pub(crate) fn has_checks(&self) -> bool {
        match self {
            Constraints::None => false,
            Constraints::String {
                min_length,
                max_length,
                pattern,
                enumeration,
            } => {
                min_length.is_some()
                    || max_length.is_some()
                    || pattern.is_some()
                    || !enumeration.is_empty()
            }
            Constraints::Integer {
                minimum,
                maximum,
                exclusive_minimum,
                exclusive_maximum,
                multiple_of,
                enumeration,
            } => {
                minimum.is_some()
                    || maximum.is_some()
                    || *exclusive_minimum
                    || *exclusive_maximum
                    || multiple_of.is_some()
                    || !enumeration.is_empty()
            }
            Constraints::Number {
                minimum,
                maximum,
                exclusive_minimum,
                exclusive_maximum,
                multiple_of,
            } => {
                minimum.is_some()
                    || maximum.is_some()
                    || *exclusive_minimum
                    || *exclusive_maximum
                    || multiple_of.is_some()
            }
            Constraints::Array {
                min_items,
                max_items,
                unique_items,
            } => min_items.is_some() || max_items.is_some() || *unique_items,
            Constraints::Nested | Constraints::VecNested => true,
        }
    }

    /// Number of independent validation checks this constraint will emit.
    pub(crate) fn n_checks(&self) -> usize {
        match self {
            Constraints::None => 0,
            Constraints::String {
                min_length,
                max_length,
                pattern,
                enumeration,
            } => [
                min_length.is_some(),
                max_length.is_some(),
                pattern.is_some(),
                !enumeration.is_empty(),
            ]
            .iter()
            .filter(|&&x| x)
            .count(),
            Constraints::Integer {
                minimum,
                maximum,
                multiple_of,
                enumeration,
                ..
            } => [
                minimum.is_some(),
                maximum.is_some(),
                multiple_of.is_some(),
                !enumeration.is_empty(),
            ]
            .iter()
            .filter(|&&x| x)
            .count(),
            Constraints::Number {
                minimum,
                maximum,
                multiple_of,
                ..
            } => [minimum.is_some(), maximum.is_some(), multiple_of.is_some()]
                .iter()
                .filter(|&&x| x)
                .count(),
            Constraints::Array {
                min_items,
                max_items,
                unique_items,
            } => [min_items.is_some(), max_items.is_some(), *unique_items]
                .iter()
                .filter(|&&x| x)
                .count(),
            Constraints::Nested | Constraints::VecNested => 1,
        }
    }
}
