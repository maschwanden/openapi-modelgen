//! Top-level `oneOf` schemas to union entities, and the pass that prunes them.

use std::collections::HashSet;

use openapiv3::{ReferenceOr, Schema, SchemaKind};

use super::{Parser, is_local_schema_ref, resolve_ref_name};
use crate::{
    Entity, UnionDef, UnionVariant,
    diagnostic::{Severity, record},
    ident::to_type_ident,
};

impl Parser {
    /// Post-pass that makes every `oneOf` union sound.
    ///
    /// [`parse_one_of`] sees one schema at a time, so it cannot tell whether a
    /// member `$ref` names a type that was actually generated, nor what shape that
    /// type has. With the full entity list this pass:
    ///
    /// * drops variants whose member produced no type (e.g. an `allOf` schema),
    ///   which would otherwise reference a nonexistent Rust type;
    /// * drops non-struct variants of a *tagged* union, since serde's internally tagged
    ///   representation requires each payload to serialize as a map, and a variant
    ///   wrapping an enum or scalar silently round-trips to garbage;
    /// * drops variants whose PascalCase name collides with an earlier one, which
    ///   would emit a duplicate enum variant;
    /// * removes the discriminator property from every member struct of a tagged
    ///   union (see [`strip_discriminator_properties`]);
    /// * removes a union left with no usable variants at all.
    pub(super) fn resolve_unions(&mut self, entities: &mut Vec<Entity>) {
        let mut struct_names = HashSet::new();
        let mut generated_names = HashSet::new();
        for entity in entities.iter() {
            match entity {
                Entity::Struct(s) => {
                    struct_names.insert(s.name.clone());
                    generated_names.insert(s.name.clone());
                }
                Entity::Enum(e) => {
                    generated_names.insert(e.name.clone());
                }
                Entity::Union(u) => {
                    generated_names.insert(u.name.clone());
                }
            }
        }

        // Unions that already got a per-member diagnostic; if such a union ends up
        // empty, the member reports explain it and a schema-level drop would just
        // restate them.
        let mut reported = HashSet::new();
        // `(member type, discriminator property)` pairs to strip afterwards.
        let mut tags_to_strip = Vec::new();

        for entity in entities.iter_mut() {
            let Entity::Union(union_def) = entity else {
                continue;
            };
            let path = format!("components.schemas.{}", union_def.name);
            let tagged = union_def.tag.is_some();

            let mut seen = HashSet::new();
            let mut kept = Vec::new();
            for variant in std::mem::take(&mut union_def.variants) {
                let drop_reason = if !generated_names.contains(&variant.inner_type) {
                    Some(format!(
                        "member `{}` has no generated type; the variant was dropped",
                        variant.inner_type
                    ))
                } else if tagged && !struct_names.contains(&variant.inner_type) {
                    Some(format!(
                        "member `{}` is not an object schema, so a discriminated union cannot wrap it; the variant was dropped",
                        variant.inner_type
                    ))
                } else if !seen.insert(variant.variant_name.clone()) {
                    Some(format!(
                        "member `{}` maps to variant `{}`, which is already taken; the variant was dropped",
                        variant.inner_type, variant.variant_name
                    ))
                } else {
                    None
                };

                match drop_reason {
                    Some(reason) => {
                        reported.insert(union_def.name.clone());
                        self.record(Severity::Dropped, path.clone(), "oneOf member", reason);
                    }
                    None => kept.push(variant),
                }
            }

            if let Some(tag) = &union_def.tag {
                for variant in &kept {
                    tags_to_strip.push((variant.inner_type.clone(), tag.clone()));
                }
            }
            union_def.variants = kept;
        }

        strip_discriminator_properties(entities, &tags_to_strip);

        entities.retain(|entity| {
            let Entity::Union(union_def) = entity else {
                return true;
            };
            if !union_def.variants.is_empty() {
                return true;
            }
            if !reported.contains(&union_def.name) {
                self.record(
                    Severity::Dropped,
                    format!("components.schemas.{}", union_def.name),
                    "oneOf",
                    "no member of the oneOf produced a usable variant; no type was generated",
                );
            }
            false
        });
    }

    /// Parse a top-level `oneOf` schema into a union entity.
    ///
    /// Each member is expected to be a `$ref` to a local component schema; each
    /// becomes an enum variant wrapping the referenced type. Inline (non-`$ref`)
    /// and non-local members are out of scope, so they are skipped. If a
    /// `discriminator` is present the union is internally tagged, and every variant
    /// gets a wire value: the `mapping` key when one points at the member,
    /// otherwise the member's schema name, which is what OpenAPI implies.
    ///
    /// Members that survive here are still only *candidates*: whether the
    /// referenced type exists and has a usable shape is settled by
    /// [`resolve_unions`], which sees the whole entity list.
    ///
    /// Per-member skip diagnostics are buffered and only flushed when a union is
    /// actually produced (a mix of usable variants and skipped members). If *no*
    /// variant survives, nothing is recorded here so the single schema-level drop in
    /// `diagnose_unsupported_schema` fires instead, avoiding a double report for
    /// the same schema.
    pub(super) fn parse_one_of(&mut self, name: &str, schema: &Schema) -> Option<Entity> {
        let SchemaKind::OneOf { one_of } = &schema.schema_kind else {
            return None;
        };

        let path = format!("components.schemas.{name}");
        let discriminator = schema.schema_data.discriminator.as_ref();

        let mut variants = Vec::new();
        let mut skipped = Vec::new();
        for member in one_of {
            let ReferenceOr::Reference { reference } = member else {
                record(
                    &mut skipped,
                    Severity::Dropped,
                    path.clone(),
                    "inline oneOf member",
                    "inline (non-$ref) oneOf members are not supported; this variant was skipped",
                );
                continue;
            };
            // A non-local $ref has no generated type and no valid Rust name; using
            // it would emit the raw ref string as an identifier.
            if !is_local_schema_ref(reference) {
                record(
                    &mut skipped,
                    Severity::Dropped,
                    path.clone(),
                    "external oneOf member",
                    format!(
                        "oneOf member $ref `{reference}` is not a local component schema; the variant was dropped"
                    ),
                );
                continue;
            }
            let ref_name = resolve_ref_name(reference);
            // The schema name is the wire value; the Rust type derives from it.
            let Some(type_name) = to_type_ident(&ref_name) else {
                record(
                    &mut skipped,
                    Severity::Dropped,
                    path.clone(),
                    "oneOf member",
                    format!(
                        "member `{ref_name}` has nothing a Rust name can be built from; \
                         the variant was dropped"
                    ),
                );
                continue;
            };

            // A discriminator mapping key overrides the wire value; without one,
            // OpenAPI uses the member's schema name, which is not necessarily the
            // PascalCase variant name, so it still has to be recorded.
            let wire_value = discriminator.map(|d| {
                d.mapping
                    .iter()
                    .find_map(|(key, target)| {
                        (resolve_ref_name(target) == ref_name).then(|| key.clone())
                    })
                    .unwrap_or_else(|| ref_name.clone())
            });

            variants.push(UnionVariant {
                variant_name: type_name.clone(),
                inner_type: type_name,
                wire_value,
            });
        }

        if variants.is_empty() {
            return None;
        }

        self.diagnostics.append(&mut skipped);
        Some(Entity::Union(UnionDef {
            name: to_type_ident(name)?,
            variants,
            tag: discriminator.map(|d| d.property_name.clone()),
        }))
    }
}

/// Remove each tagged union's discriminator property from its member structs.
///
/// `#[serde(tag = "p")]` makes serde own the `p` key: it writes `p` when
/// serializing and removes it from the payload before deserializing the
/// variant. A member struct that *also* declares `p` therefore serializes it
/// twice and fails to deserialize with `missing field \`p\``. OpenAPI specs
/// normally do declare the discriminator on each member, so the property is
/// absorbed into the tag rather than kept as a field. This is not a loss: the
/// key is still read and written on the wire.
fn strip_discriminator_properties(entities: &mut [Entity], tags_to_strip: &[(String, String)]) {
    for (member, tag) in tags_to_strip {
        for entity in entities.iter_mut() {
            let Entity::Struct(s) = entity else { continue };
            if s.name == *member {
                s.fields.retain(|field| field.name != *tag);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use crate::{Entity, UnionVariant, diagnostic::Severity, parse, parse::testutil::*};

    #[test]
    fn parse_one_of_untagged() -> Result<()> {
        let entities = spec_with_composite(
            "\
Pet:
  oneOf:
    - $ref: '#/components/schemas/Cat'
    - $ref: '#/components/schemas/Dog'",
        )?;
        let union = find_union(&entities, "Pet");
        assert_eq!(union.tag, None);
        assert_eq!(
            union.variants,
            vec![
                UnionVariant {
                    variant_name: "Cat".into(),
                    inner_type: "Cat".into(),
                    wire_value: None,
                },
                UnionVariant {
                    variant_name: "Dog".into(),
                    inner_type: "Dog".into(),
                    wire_value: None,
                },
            ]
        );

        Ok(())
    }

    #[test]
    fn parse_one_of_discriminator_no_mapping() -> Result<()> {
        let entities = spec_with_composite(
            "\
Pet:
  oneOf:
    - $ref: '#/components/schemas/Cat'
    - $ref: '#/components/schemas/Dog'
  discriminator:
    propertyName: petType",
        )?;
        let union = find_union(&entities, "Pet");
        assert_eq!(union.tag, Some("petType".into()));
        // No mapping → OpenAPI implies the member's schema name as the tag
        // value, so each variant still carries one.
        assert_eq!(
            union
                .variants
                .iter()
                .map(|v| (v.variant_name.as_str(), v.wire_value.as_deref()))
                .collect::<Vec<_>>(),
            vec![("Cat", Some("Cat")), ("Dog", Some("Dog"))]
        );

        Ok(())
    }

    /// A member whose `$ref` target has no Rust name is dropped like any other
    /// unusable member, and says so: a silently shorter union deserializes the
    /// missing variant as an error at runtime.
    #[test]
    fn parse_one_of_drops_a_member_with_no_rust_name() -> Result<()> {
        let spec = spec_with_composite_spec(
            "\
Cat:
  type: object
  properties:
    name:
      type: string
Pet:
  oneOf:
    - $ref: '#/components/schemas/Cat'
    - $ref: '#/components/schemas/!!!'",
        )?;
        let (entities, diagnostics) = parse(&spec);

        let union = find_union(&entities, "Pet");
        assert_eq!(
            union
                .variants
                .iter()
                .map(|v| v.variant_name.as_str())
                .collect::<Vec<_>>(),
            vec!["Cat"]
        );
        assert!(
            diagnostics.iter().any(|d| {
                d.severity == Severity::Dropped
                    && d.reason
                        .contains("member `!!!` has nothing a Rust name can be built from")
            }),
            "the dropped member should be reported: {diagnostics:?}"
        );

        Ok(())
    }

    /// A member whose schema name is not already PascalCase needs an explicit
    /// `#[serde(rename)]`: the implied tag value is the schema name, not the
    /// variant name derived from it. The wrapped type is the *generated* type
    /// name, which is derived from the schema name the same way the struct's
    /// own name is.
    #[test]
    fn parse_one_of_implied_tag_keeps_schema_name() -> Result<()> {
        let spec = spec_with_composite_spec(
            "\
pet_dog:
  type: object
  properties:
    name:
      type: string
Pet:
  oneOf:
    - $ref: '#/components/schemas/pet_dog'
  discriminator:
    propertyName: kind",
        )?;
        let (entities, _) = parse(&spec);
        let union = find_union(&entities, "Pet");
        assert_eq!(
            union.variants,
            vec![UnionVariant {
                variant_name: "PetDog".into(),
                inner_type: "PetDog".into(),
                wire_value: Some("pet_dog".into()),
            }]
        );

        Ok(())
    }

    #[test]
    fn parse_one_of_discriminator_with_mapping() -> Result<()> {
        let entities = spec_with_composite(
            "\
Pet:
  oneOf:
    - $ref: '#/components/schemas/Cat'
    - $ref: '#/components/schemas/Dog'
  discriminator:
    propertyName: petType
    mapping:
      cat: '#/components/schemas/Cat'
      dog: '#/components/schemas/Dog'",
        )?;
        let union = find_union(&entities, "Pet");
        assert_eq!(union.tag, Some("petType".into()));
        assert_eq!(
            union.variants,
            vec![
                UnionVariant {
                    variant_name: "Cat".into(),
                    inner_type: "Cat".into(),
                    wire_value: Some("cat".into()),
                },
                UnionVariant {
                    variant_name: "Dog".into(),
                    inner_type: "Dog".into(),
                    wire_value: Some("dog".into()),
                },
            ]
        );

        Ok(())
    }

    /// `#[serde(tag = "p")]` makes serde own the `p` key, so a member struct
    /// must not also declare it: serializing would emit `p` twice and serde
    /// strips it from the payload before deserializing the variant, which then
    /// fails with `missing field p`. The property is absorbed into the tag.
    #[test]
    fn tagged_union_strips_discriminator_property() -> Result<()> {
        let spec = spec_with_composite_spec(
            "\
Pet:
  oneOf:
    - $ref: '#/components/schemas/Cat'
    - $ref: '#/components/schemas/Dog'
  discriminator:
    propertyName: name",
        )?;
        let (entities, _) = parse(&spec);

        // `Cat`/`Dog` from the helper each have exactly one property, `name`,
        // which is the discriminator here, so both end up field-less.
        assert_eq!(
            field_names(find_struct(&entities, "Cat")),
            Vec::<&str>::new()
        );
        assert_eq!(
            field_names(find_struct(&entities, "Dog")),
            Vec::<&str>::new()
        );

        Ok(())
    }

    /// An *untagged* union has no tag key of its own, so the members keep every
    /// property they declare.
    #[test]
    fn untagged_union_keeps_member_properties() -> Result<()> {
        let spec = spec_with_composite_spec(
            "\
Pet:
  oneOf:
    - $ref: '#/components/schemas/Cat'
    - $ref: '#/components/schemas/Dog'",
        )?;
        let (entities, _) = parse(&spec);
        assert_eq!(field_names(find_struct(&entities, "Cat")), vec!["name"]);

        Ok(())
    }

    /// A non-local `$ref` member has no generated type and no valid Rust name;
    /// using it would emit the raw ref string as an identifier.
    #[test]
    fn one_of_drops_external_member() -> Result<()> {
        let spec = spec_with_composite_spec(
            "\
Pet:
  oneOf:
    - $ref: 'other.yaml#/components/schemas/Fish'
    - $ref: '#/components/schemas/Dog'",
        )?;
        let (entities, diagnostics) = parse(&spec);

        let union = find_union(&entities, "Pet");
        assert_eq!(
            union
                .variants
                .iter()
                .map(|v| v.inner_type.as_str())
                .collect::<Vec<_>>(),
            vec!["Dog"]
        );
        let d = find_diag(&diagnostics, "external oneOf member");
        assert_eq!(d.severity, Severity::Dropped);
        assert_eq!(d.path, "components.schemas.Pet");

        Ok(())
    }

    /// A member pointing at a schema the generator dropped (here an `allOf`)
    /// would reference a Rust type that was never emitted.
    #[test]
    fn one_of_drops_member_without_generated_type() -> Result<()> {
        let spec = spec_with_composite_spec(
            "\
Derived:
  allOf:
    - $ref: '#/components/schemas/Cat'
Pet:
  oneOf:
    - $ref: '#/components/schemas/Derived'
    - $ref: '#/components/schemas/Dog'",
        )?;
        let (entities, diagnostics) = parse(&spec);

        let union = find_union(&entities, "Pet");
        assert_eq!(
            union
                .variants
                .iter()
                .map(|v| v.inner_type.as_str())
                .collect::<Vec<_>>(),
            vec!["Dog"]
        );
        let d = find_diag(&diagnostics, "oneOf member");
        assert_eq!(d.severity, Severity::Dropped);
        assert!(d.reason.contains("no generated type"), "{}", d.reason);

        Ok(())
    }

    /// serde's internally tagged representation requires each variant's payload
    /// to serialize as a map. A variant wrapping an enum compiles but round-trips
    /// to garbage, so it is dropped instead.
    #[test]
    fn tagged_one_of_drops_non_object_member() -> Result<()> {
        let spec = spec_with_composite_spec(
            "\
Status:
  type: string
  enum: [a, b]
Pet:
  oneOf:
    - $ref: '#/components/schemas/Status'
    - $ref: '#/components/schemas/Dog'
  discriminator:
    propertyName: kind",
        )?;
        let (entities, diagnostics) = parse(&spec);

        let union = find_union(&entities, "Pet");
        assert_eq!(
            union
                .variants
                .iter()
                .map(|v| v.inner_type.as_str())
                .collect::<Vec<_>>(),
            vec!["Dog"]
        );
        let d = find_diag(&diagnostics, "oneOf member");
        assert!(d.reason.contains("not an object schema"), "{}", d.reason);

        // The same member is fine in an *untagged* union, which imposes no
        // shape requirement.
        let untagged = spec_with_composite_spec(
            "\
Status:
  type: string
  enum: [a, b]
Pet:
  oneOf:
    - $ref: '#/components/schemas/Status'
    - $ref: '#/components/schemas/Dog'",
        )?;
        assert_eq!(find_union(&parse(&untagged).0, "Pet").variants.len(), 2);

        Ok(())
    }

    /// Two members whose names PascalCase to the same variant would emit a
    /// duplicate enum variant, which does not compile.
    #[test]
    fn one_of_drops_colliding_variant() -> Result<()> {
        let spec = spec_with_composite_spec(
            "\
pet_dog:
  type: object
  properties:
    a:
      type: string
PetDog:
  type: object
  properties:
    b:
      type: string
Pet:
  oneOf:
    - $ref: '#/components/schemas/pet_dog'
    - $ref: '#/components/schemas/PetDog'",
        )?;
        let (entities, diagnostics) = parse(&spec);

        assert_eq!(find_union(&entities, "Pet").variants.len(), 1);
        let d = find_diag(&diagnostics, "oneOf member");
        assert!(d.reason.contains("already taken"), "{}", d.reason);

        Ok(())
    }

    /// A union whose every member is unusable is removed rather than emitted as
    /// an uninhabited enum, and it is reported exactly once.
    #[test]
    fn one_of_with_no_usable_member_is_removed() -> Result<()> {
        let spec = spec_with_composite_spec(
            "\
Derived:
  allOf:
    - $ref: '#/components/schemas/Cat'
Pet:
  oneOf:
    - $ref: '#/components/schemas/Derived'",
        )?;
        let (entities, diagnostics) = parse(&spec);

        assert!(!entities.iter().any(|e| matches!(e, Entity::Union(_))));
        // One member report; no additional schema-level `oneOf` drop restating it.
        assert_eq!(
            count_construct(&diagnostics, "oneOf member"),
            1,
            "{diagnostics:?}"
        );
        assert_eq!(count_construct(&diagnostics, "oneOf"), 0, "{diagnostics:?}");

        Ok(())
    }

    /// An all-inline `oneOf` (no `$ref` members) yields exactly ONE diagnostic,
    /// the schema-level drop, not one per skipped member plus the drop.
    #[test]
    fn diagnostic_all_inline_one_of_single() -> Result<()> {
        let spec = spec_with_composite_spec(
            "\
Pet:
  oneOf:
    - type: object
      properties:
        a:
          type: string
    - type: object
      properties:
        b:
          type: string",
        )?;
        let (entities, diagnostics) = parse(&spec);

        // No union produced, and exactly one schema-level drop for the oneOf.
        assert!(!entities.iter().any(|e| matches!(e, Entity::Union(_))));
        assert_eq!(count_construct(&diagnostics, "oneOf"), 1, "{diagnostics:?}");
        assert_eq!(
            count_construct(&diagnostics, "inline oneOf member"),
            0,
            "{diagnostics:?}"
        );

        Ok(())
    }

    /// A mixed `oneOf` (one `$ref` + one inline member) produces the union AND
    /// one diagnostic per skipped inline member (no schema-level drop).
    #[test]
    fn diagnostic_mixed_one_of_per_member() -> Result<()> {
        let spec = spec_with_composite_spec(
            "\
Pet:
  oneOf:
    - $ref: '#/components/schemas/Cat'
    - type: object
      properties:
        b:
          type: string",
        )?;
        let (entities, diagnostics) = parse(&spec);
        assert!(
            entities
                .iter()
                .any(|e| matches!(e, Entity::Union(u) if u.name == "Pet")),
            "union should still be produced"
        );
        assert_eq!(
            count_construct(&diagnostics, "inline oneOf member"),
            1,
            "{diagnostics:?}"
        );
        assert_eq!(count_construct(&diagnostics, "oneOf"), 0, "{diagnostics:?}");

        Ok(())
    }
}
