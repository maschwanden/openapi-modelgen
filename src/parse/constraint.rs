//! Validation keywords to the [`Constraints`] a field carries.

use openapiv3::{Schema, SchemaKind, Type};

use crate::Constraints;

/// Extract validation constraints from an inline schema.
pub(super) fn extract_constraints(schema: &Schema) -> Constraints {
    match &schema.schema_kind {
        SchemaKind::Type(Type::String(s)) => {
            let enumeration: Vec<String> = s.enumeration.iter().filter_map(Clone::clone).collect();
            if s.min_length.is_none()
                && s.max_length.is_none()
                && s.pattern.is_none()
                && enumeration.is_empty()
            {
                return Constraints::None;
            }
            Constraints::String {
                min_length: s.min_length,
                max_length: s.max_length,
                pattern: s.pattern.clone(),
                enumeration,
            }
        }
        SchemaKind::Type(Type::Integer(i)) => {
            let enumeration: Vec<i64> = i.enumeration.iter().filter_map(|v| *v).collect();
            if i.minimum.is_none()
                && i.maximum.is_none()
                && !i.exclusive_minimum
                && !i.exclusive_maximum
                && i.multiple_of.is_none()
                && enumeration.is_empty()
            {
                return Constraints::None;
            }
            Constraints::Integer {
                minimum: i.minimum,
                maximum: i.maximum,
                exclusive_minimum: i.exclusive_minimum,
                exclusive_maximum: i.exclusive_maximum,
                multiple_of: i.multiple_of,
                enumeration,
            }
        }
        SchemaKind::Type(Type::Number(n)) => {
            if n.minimum.is_none()
                && n.maximum.is_none()
                && !n.exclusive_minimum
                && !n.exclusive_maximum
                && n.multiple_of.is_none()
            {
                return Constraints::None;
            }
            Constraints::Number {
                minimum: n.minimum,
                maximum: n.maximum,
                exclusive_minimum: n.exclusive_minimum,
                exclusive_maximum: n.exclusive_maximum,
                multiple_of: n.multiple_of,
            }
        }
        SchemaKind::Type(Type::Array(a)) => {
            if a.min_items.is_none() && a.max_items.is_none() && !a.unique_items {
                return Constraints::None;
            }
            Constraints::Array {
                min_items: a.min_items,
                max_items: a.max_items,
                unique_items: a.unique_items,
            }
        }
        _ => Constraints::None,
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use crate::{Constraints, parse, parse::testutil::*};

    #[test]
    fn extract_constraints() -> Result<()> {
        // String pattern
        let spec = spec_with_schema(
            "\
type: object
required: [s]
properties:
  s:
    type: string
    pattern: '^[A-Z]{3}$'",
        )?;
        assert_eq!(
            first_struct_fields(&parse(&spec).0)[0].constraints,
            Constraints::String {
                min_length: None,
                max_length: None,
                pattern: Some("^[A-Z]{3}$".into()),
                enumeration: vec![],
            }
        );

        // Integer multipleOf + enum
        let spec = spec_with_schema(
            "\
type: object
required: [v]
properties:
  v:
    type: integer
    format: int64
    multipleOf: 5
    enum: [5, 10, 15]",
        )?;
        assert_eq!(
            first_struct_fields(&parse(&spec).0)[0].constraints,
            Constraints::Integer {
                minimum: None,
                maximum: None,
                exclusive_minimum: false,
                exclusive_maximum: false,
                multiple_of: Some(5),
                enumeration: vec![5, 10, 15],
            }
        );

        // Number exclusive min/max
        let spec = spec_with_schema(
            "\
type: object
required: [n]
properties:
  n:
    type: number
    minimum: 0.0
    maximum: 100.0
    exclusiveMinimum: true
    exclusiveMaximum: true",
        )?;
        assert_eq!(
            first_struct_fields(&parse(&spec).0)[0].constraints,
            Constraints::Number {
                minimum: Some(0.0),
                maximum: Some(100.0),
                exclusive_minimum: true,
                exclusive_maximum: true,
                multiple_of: None,
            }
        );

        // Array uniqueItems
        let spec = spec_with_schema(
            "\
type: object
required: [tags]
properties:
  tags:
    type: array
    items:
      type: string
    uniqueItems: true",
        )?;
        assert_eq!(
            first_struct_fields(&parse(&spec).0)[0].constraints,
            Constraints::Array {
                min_items: None,
                max_items: None,
                unique_items: true,
            }
        );

        // No constraints
        let spec = spec_with_schema(
            "\
type: object
required: [name]
properties:
  name:
    type: string",
        )?;
        assert_eq!(
            first_struct_fields(&parse(&spec).0)[0].constraints,
            Constraints::None
        );

        // Type mapping: boolean → bool
        let spec = spec_with_schema(
            "\
type: object
required: [active]
properties:
  active:
    type: boolean",
        )?;
        assert_eq!(first_struct_fields(&parse(&spec).0)[0].rust_type, "bool");

        // Type mapping: number → f64
        let spec = spec_with_schema(
            "\
type: object
required: [score]
properties:
  score:
    type: number",
        )?;
        assert_eq!(first_struct_fields(&parse(&spec).0)[0].rust_type, "f64");

        Ok(())
    }
}
