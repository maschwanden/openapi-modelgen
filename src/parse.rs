use openapiv3::{
    IntegerFormat, OpenAPI, Operation, ReferenceOr, Schema, SchemaKind, StringFormat, Type,
    VariantOrUnknownOrEmpty,
};

use crate::{
    Constraints, Diagnostic, Entity, EntityKind, EnumDef, Field, StructDef, UnionDef, UnionVariant,
    diagnostics::{Severity, record},
};

/// Parse an OpenAPI spec into a list of entities, discarding diagnostics.
///
/// Prefer [`parse_with_diagnostics`] (or [`crate::generate`]) when you need to
/// know which spec constructs could not be generated.
pub fn parse(spec: &OpenAPI) -> Vec<Entity> {
    let mut diagnostics = Vec::new();
    parse_with_diagnostics(spec, &mut diagnostics)
}

/// Parse an OpenAPI spec into a list of entities, recording a [`Diagnostic`]
/// for every construct that is dropped or degraded.
pub fn parse_with_diagnostics(spec: &OpenAPI, diagnostics: &mut Vec<Diagnostic>) -> Vec<Entity> {
    let mut entities = Vec::new();

    if let Some(components) = &spec.components {
        for (name, ref_or) in &components.schemas {
            let schema = match ref_or {
                ReferenceOr::Item(schema) => schema,
                ReferenceOr::Reference { reference } => {
                    record(
                        diagnostics,
                        Severity::Dropped,
                        format!("components.schemas.{name}"),
                        "$ref schema",
                        format!(
                            "top-level schema is a $ref to `{reference}`; no type was generated"
                        ),
                    );
                    continue;
                }
            };

            let entity = parse_schema(name, schema, diagnostics)
                .or_else(|| parse_one_of(name, schema, diagnostics))
                .or_else(|| parse_enum(name, schema).map(Entity::Enum));

            match entity {
                Some(entity) => entities.push(entity),
                None => diagnose_unsupported_schema(name, schema, diagnostics),
            }
        }
    }

    for (path, path_item_ref) in spec.paths.iter() {
        let path_item = match path_item_ref {
            ReferenceOr::Item(item) => item,
            ReferenceOr::Reference { reference } => {
                record(
                    diagnostics,
                    Severity::Dropped,
                    path.clone(),
                    "$ref path item",
                    format!("path item is a $ref to `{reference}`; no operations were generated"),
                );
                continue;
            }
        };

        let ops: [(&str, &Option<Operation>); 5] = [
            ("get", &path_item.get),
            ("put", &path_item.put),
            ("post", &path_item.post),
            ("patch", &path_item.patch),
            ("delete", &path_item.delete),
        ];
        for (method, op) in ops {
            let Some(op) = op else { continue };
            diagnose_operation_bodies(op, spec.components.as_ref(), method, path, diagnostics);
            if let Some(entity) =
                parse_query(op, spec.components.as_ref(), method, path, diagnostics)
            {
                entities.push(entity);
            }
        }
    }

    entities
}

/// Record a diagnostic for a top-level schema whose kind the generator does not
/// turn into a type. `oneOf` member-level losses are reported inside
/// [`parse_one_of`]; this covers the schema-level drop.
fn diagnose_unsupported_schema(name: &str, schema: &Schema, diagnostics: &mut Vec<Diagnostic>) {
    let path = format!("components.schemas.{name}");
    let (construct, reason): (&str, &str) = match &schema.schema_kind {
        SchemaKind::AllOf { .. } => (
            "allOf",
            "allOf composition is not supported; no type was generated",
        ),
        SchemaKind::AnyOf { .. } => (
            "anyOf",
            "anyOf composition is not supported; no type was generated",
        ),
        SchemaKind::Not { .. } => (
            "not",
            "`not` schemas are not supported; no type was generated",
        ),
        SchemaKind::Any(_) => (
            "free-form schema",
            "free-form/ambiguous schema; no type was generated",
        ),
        SchemaKind::OneOf { .. } => (
            "oneOf",
            "oneOf has no supported $ref members; no type was generated",
        ),
        SchemaKind::Type(Type::String(_)) => (
            "string schema",
            "top-level string type alias (or non-string enum) is not generated as a distinct type",
        ),
        SchemaKind::Type(Type::Integer(_)) => (
            "integer schema",
            "top-level integer schema (including integer enums) is not generated as a distinct type",
        ),
        SchemaKind::Type(Type::Number(_)) => (
            "number schema",
            "top-level number type alias is not generated as a distinct type",
        ),
        SchemaKind::Type(Type::Boolean(_)) => (
            "boolean schema",
            "top-level boolean type alias is not generated as a distinct type",
        ),
        SchemaKind::Type(Type::Array(_)) => (
            "array schema",
            "top-level array type alias is not generated as a distinct type",
        ),
        // Objects are always handled by `parse_schema`; nothing to report.
        SchemaKind::Type(Type::Object(_)) => return,
    };
    record(diagnostics, Severity::Dropped, path, construct, reason);
}

/// Whether any media type in a body carries an *inline* schema.
///
/// A `$ref` content schema resolves to a component we generate, so it is not a
/// loss; only an inline schema (`ReferenceOr::Item`) has no generated type.
fn content_has_inline_schema<'a>(
    content: impl IntoIterator<Item = &'a openapiv3::MediaType>,
) -> bool {
    content
        .into_iter()
        .any(|media| matches!(media.schema, Some(ReferenceOr::Item(_))))
}

/// Record diagnostics for operation request/response bodies, which are never
/// parsed into types.
///
/// A body is a loss only when its content schema is *inline*; a `$ref` content
/// schema points at a component we do generate. `$ref` bodies/responses are
/// resolved against `components`, then the same inline check applies — an
/// unresolvable `$ref` (external or missing) is itself a genuine loss.
fn diagnose_operation_bodies(
    op: &Operation,
    components: Option<&openapiv3::Components>,
    method: &str,
    path: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let location = format!("{} {path}", method.to_uppercase());

    // Request body: a loss only when its content schema is inline. A `$ref`
    // body is resolved first; an unresolvable `$ref` is itself a loss.
    if let Some(ref_or) = &op.request_body {
        let resolved = match ref_or {
            ReferenceOr::Item(body) => Some(body),
            ReferenceOr::Reference { reference } => {
                let body = resolve_request_body(reference, components);
                if body.is_none() {
                    record(
                        diagnostics,
                        Severity::Dropped,
                        location.clone(),
                        "request body",
                        format!("could not resolve request body $ref `{reference}`"),
                    );
                }
                body
            }
        };
        if let Some(body) = resolved
            && content_has_inline_schema(body.content.values())
        {
            record(
                diagnostics,
                Severity::Dropped,
                location.clone(),
                "request body",
                "inline request body schema is not generated as a named type",
            );
        }
    }

    // Responses (every status plus the `default` response).
    let mut response_has_inline_schema = false;
    for response_ref in op.responses.responses.values().chain(&op.responses.default) {
        match response_ref {
            ReferenceOr::Item(response) => {
                response_has_inline_schema |= content_has_inline_schema(response.content.values());
            }
            ReferenceOr::Reference { reference } => match resolve_response(reference, components) {
                Some(response) => {
                    response_has_inline_schema |=
                        content_has_inline_schema(response.content.values());
                }
                None => record(
                    diagnostics,
                    Severity::Dropped,
                    location.clone(),
                    "response body",
                    format!("could not resolve response $ref `{reference}`"),
                ),
            },
        }
    }
    if response_has_inline_schema {
        record(
            diagnostics,
            Severity::Dropped,
            location,
            "response body",
            "inline response body schema is not generated as a named type",
        );
    }
}

fn parse_schema(name: &str, schema: &Schema, diagnostics: &mut Vec<Diagnostic>) -> Option<Entity> {
    let SchemaKind::Type(Type::Object(obj)) = &schema.schema_kind else {
        return None;
    };

    // A map-shaped object (`additionalProperties` with an empty property set) is
    // generated as an empty struct, silently dropping the map value type.
    if has_meaningful_additional_properties(obj) {
        record(
            diagnostics,
            Severity::Degraded,
            format!("components.schemas.{name}"),
            "additionalProperties",
            "additionalProperties is ignored; map values are not represented in the generated struct",
        );
    }

    let mut fields = Vec::new();
    let mut enums = Vec::new();

    for (field_name, field_ref) in &obj.properties {
        let required = obj.required.contains(field_name);
        let context = format!("{name}.{field_name}");

        // Check for inline string enums
        let (rust_type, nullable) = match field_ref {
            ReferenceOr::Item(field_schema) => {
                if let Some(enum_def) = parse_enum(field_name, field_schema) {
                    let ty = enum_def.name.clone();
                    let nullable = field_schema.schema_data.nullable;
                    enums.push(enum_def);
                    (ty, nullable)
                } else {
                    map_schema_to_type(field_schema, &context, diagnostics)
                }
            }
            ReferenceOr::Reference { .. } => resolve_field_type(field_ref, &context, diagnostics),
        };

        // Extract default value (only for supported types).
        let is_enum_field = enums.iter().any(|e| e.name == rust_type);
        let default_value = match field_ref {
            ReferenceOr::Item(field_schema) => extract_default(
                &field_schema.schema_data.default,
                &rust_type,
                is_enum_field,
                name,
                field_name,
                diagnostics,
            ),
            ReferenceOr::Reference { .. } => None,
        };

        let has_default = default_value.is_some();
        let is_optional = if has_default {
            nullable
        } else {
            !required || nullable
        };

        // If the field was converted to an enum type, serde handles validation —
        // no runtime constraints needed.
        let constraints = if is_enum_field {
            Constraints::None
        } else {
            match field_ref {
                ReferenceOr::Reference { .. } => Constraints::Nested,
                ReferenceOr::Item(field_schema) => {
                    if let SchemaKind::Type(Type::Array(arr)) = &field_schema.schema_kind {
                        if let Some(ReferenceOr::Reference { .. }) = &arr.items {
                            Constraints::VecNested
                        } else {
                            extract_constraints(field_schema)
                        }
                    } else {
                        extract_constraints(field_schema)
                    }
                }
            }
        };

        fields.push(Field {
            name: field_name.clone(),
            rust_type,
            is_optional,
            constraints,
            default_value,
        });
    }

    Some(Entity::Struct(StructDef {
        name: name.to_string(),
        kind: EntityKind::Schema,
        fields,
        enums,
    }))
}

/// If the schema is a string with enum values, generate an EnumDef.
fn parse_enum(field_name: &str, schema: &Schema) -> Option<EnumDef> {
    if let SchemaKind::Type(Type::String(s)) = &schema.schema_kind {
        let values: Vec<String> = s.enumeration.iter().filter_map(Clone::clone).collect();
        if values.is_empty() {
            return None;
        }
        let enum_name = to_pascal_case(field_name);
        let variants = values
            .into_iter()
            .map(|v| {
                let pascal = to_pascal_case(&v);
                (pascal, v)
            })
            .collect();
        Some(EnumDef {
            name: enum_name,
            variants,
        })
    } else {
        None
    }
}

/// Parse a top-level `oneOf` schema into a union entity.
///
/// Each member is expected to be a `$ref` to another schema; each becomes an
/// enum variant wrapping the referenced type. Inline (non-`$ref`) members are
/// out of scope for now — they are skipped with a warning. If a `discriminator`
/// is present, the union is internally tagged and any `mapping` entries rename
/// the corresponding variants on the wire.
fn parse_one_of(name: &str, schema: &Schema, diagnostics: &mut Vec<Diagnostic>) -> Option<Entity> {
    let SchemaKind::OneOf { one_of } = &schema.schema_kind else {
        return None;
    };

    let discriminator = schema.schema_data.discriminator.as_ref();

    let mut variants = Vec::new();
    for member in one_of {
        let ReferenceOr::Reference { reference } = member else {
            record(
                diagnostics,
                Severity::Dropped,
                format!("components.schemas.{name}"),
                "inline oneOf member",
                "inline (non-$ref) oneOf members are not supported; this variant was skipped",
            );
            continue;
        };
        let ref_name = resolve_ref_name(reference);

        // If a discriminator mapping points at this member's ref, use the
        // mapping key as the wire value.
        let wire_value = discriminator.and_then(|d| {
            d.mapping.iter().find_map(|(key, target)| {
                (resolve_ref_name(target) == ref_name).then(|| key.clone())
            })
        });

        variants.push(UnionVariant {
            variant_name: to_pascal_case(&ref_name),
            inner_type: ref_name,
            wire_value,
        });
    }

    if variants.is_empty() {
        return None;
    }

    Some(Entity::Union(UnionDef {
        name: name.to_string(),
        variants,
        tag: discriminator.map(|d| d.property_name.clone()),
    }))
}

fn parse_query(
    op: &Operation,
    components: Option<&openapiv3::Components>,
    method: &str,
    path: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<Entity> {
    let location = format!("{} {path}", method.to_uppercase());

    let mut query_params = Vec::new();
    for p in &op.parameters {
        let param = match p {
            ReferenceOr::Item(param) => param,
            ReferenceOr::Reference { reference } => {
                match resolve_parameter_ref(reference, components) {
                    Some(param) => param,
                    None => {
                        record(
                            diagnostics,
                            Severity::Dropped,
                            location.clone(),
                            "$ref parameter",
                            format!(
                                "could not resolve parameter $ref `{reference}`; parameter dropped"
                            ),
                        );
                        continue;
                    }
                }
            }
        };
        match param {
            openapiv3::Parameter::Query { .. } => query_params.push(param),
            // Path parameters live in the URL, not the query struct — excluded by design.
            openapiv3::Parameter::Path { .. } => {}
            openapiv3::Parameter::Header { parameter_data, .. } => record(
                diagnostics,
                Severity::Dropped,
                format!("{location}#{}", parameter_data.name),
                "header parameter",
                "header parameters are not generated",
            ),
            openapiv3::Parameter::Cookie { parameter_data, .. } => record(
                diagnostics,
                Severity::Dropped,
                format!("{location}#{}", parameter_data.name),
                "cookie parameter",
                "cookie parameters are not generated",
            ),
        }
    }

    if query_params.is_empty() {
        return None;
    }

    let struct_name = match &op.operation_id {
        Some(id) => format!("{}Query", to_pascal_case(id)),
        None => format!("{}Query", query_name_from_path(method, path)),
    };

    let mut fields = Vec::new();

    for param in &query_params {
        let data = parameter_data(param);
        let param_path = format!("{location}#{}", data.name);
        let openapiv3::ParameterSchemaOrContent::Schema(schema_ref) = &data.format else {
            record(
                diagnostics,
                Severity::Dropped,
                param_path,
                "content parameter",
                "parameter uses `content` instead of `schema`; not generated",
            );
            continue;
        };
        let (rust_type, nullable) = resolve_schema_ref(schema_ref, &param_path, diagnostics);

        let default_value = match schema_ref {
            ReferenceOr::Item(schema) => extract_default(
                &schema.schema_data.default,
                &rust_type,
                false,
                &struct_name,
                &data.name,
                diagnostics,
            ),
            ReferenceOr::Reference { .. } => None,
        };

        let has_default = default_value.is_some();
        let is_optional = if has_default {
            nullable
        } else {
            !data.required || nullable
        };

        let constraints = match schema_ref {
            ReferenceOr::Reference { .. } => Constraints::Nested,
            ReferenceOr::Item(schema) => extract_constraints(schema),
        };

        fields.push(Field {
            name: data.name.clone(),
            rust_type,
            is_optional,
            constraints,
            default_value,
        });
    }

    Some(Entity::Struct(StructDef {
        name: struct_name,
        kind: EntityKind::Query,
        fields,
        enums: Vec::new(),
    }))
}

/// Extract validation constraints from an inline schema.
fn extract_constraints(schema: &Schema) -> Constraints {
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

/// Extract the common `ParameterData` from any parameter variant (query, header, path, cookie).
fn parameter_data(param: &openapiv3::Parameter) -> &openapiv3::ParameterData {
    match param {
        openapiv3::Parameter::Query { parameter_data, .. }
        | openapiv3::Parameter::Header { parameter_data, .. }
        | openapiv3::Parameter::Path { parameter_data, .. }
        | openapiv3::Parameter::Cookie { parameter_data, .. } => parameter_data,
    }
}

/// Resolve a `$ref` string like `#/components/parameters/Foo` to the actual parameter.
fn resolve_parameter_ref<'a>(
    reference: &str,
    components: Option<&'a openapiv3::Components>,
) -> Option<&'a openapiv3::Parameter> {
    let name = reference.strip_prefix("#/components/parameters/")?;
    let params = &components.as_ref()?.parameters;
    match params.get(name)? {
        ReferenceOr::Item(p) => Some(p),
        ReferenceOr::Reference { .. } => None,
    }
}

/// Resolve a `$ref` like `#/components/requestBodies/Foo` to the request body.
fn resolve_request_body<'a>(
    reference: &str,
    components: Option<&'a openapiv3::Components>,
) -> Option<&'a openapiv3::RequestBody> {
    let name = reference.strip_prefix("#/components/requestBodies/")?;
    match components?.request_bodies.get(name)? {
        ReferenceOr::Item(b) => Some(b),
        ReferenceOr::Reference { .. } => None,
    }
}

/// Resolve a `$ref` like `#/components/responses/Foo` to the response.
fn resolve_response<'a>(
    reference: &str,
    components: Option<&'a openapiv3::Components>,
) -> Option<&'a openapiv3::Response> {
    let name = reference.strip_prefix("#/components/responses/")?;
    match components?.responses.get(name)? {
        ReferenceOr::Item(r) => Some(r),
        ReferenceOr::Reference { .. } => None,
    }
}

/// Extract the schema name from a `$ref` string (e.g. `#/components/schemas/Foo` -> `Foo`).
fn resolve_ref_name(reference: &str) -> String {
    reference
        .strip_prefix("#/components/schemas/")
        .unwrap_or(reference)
        .to_string()
}

/// Whether a `$ref` points at a local component schema (the only kind we can
/// turn into a valid Rust type name).
fn is_local_schema_ref(reference: &str) -> bool {
    reference.starts_with("#/components/schemas/")
}

/// Record a diagnostic for a `$ref` that does not point at a local component
/// schema — the generated type name is the raw ref string and will not compile.
fn diagnose_ref(reference: &str, context: &str, diagnostics: &mut Vec<Diagnostic>) {
    if !is_local_schema_ref(reference) {
        record(
            diagnostics,
            Severity::Degraded,
            context.to_string(),
            "external $ref",
            format!(
                "$ref `{reference}` is not a local component schema; the generated type name likely will not compile"
            ),
        );
    }
}

/// Resolve a field's `ReferenceOr<Schema>` to a `(rust_type, nullable)` pair.
fn resolve_field_type(
    field_ref: &ReferenceOr<Box<Schema>>,
    context: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> (String, bool) {
    match field_ref {
        ReferenceOr::Reference { reference } => {
            diagnose_ref(reference, context, diagnostics);
            (resolve_ref_name(reference), false)
        }
        ReferenceOr::Item(schema) => map_schema_to_type(schema, context, diagnostics),
    }
}

/// Resolve a schema reference (used for parameters) to a `(rust_type, nullable)` pair.
fn resolve_schema_ref(
    schema_ref: &ReferenceOr<Schema>,
    context: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> (String, bool) {
    match schema_ref {
        ReferenceOr::Reference { reference } => {
            diagnose_ref(reference, context, diagnostics);
            (resolve_ref_name(reference), false)
        }
        ReferenceOr::Item(schema) => map_schema_to_type(schema, context, diagnostics),
    }
}

/// Whether an object schema carries an `additionalProperties` that we drop
/// (a schema value or `true`). `additionalProperties: false` carries no data.
fn has_meaningful_additional_properties(obj: &openapiv3::ObjectType) -> bool {
    matches!(
        obj.additional_properties,
        Some(openapiv3::AdditionalProperties::Schema(_))
            | Some(openapiv3::AdditionalProperties::Any(true))
    )
}

/// Map an OpenAPI schema to a Rust type string and nullable flag.
///
/// Handles string (with date-time format), integer (i32/i64), number (f64),
/// boolean, and array types. Falls back to `serde_json::Value` for anything
/// else, recording a [`Severity::Degraded`] diagnostic at `context`.
fn map_schema_to_type(
    schema: &Schema,
    context: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> (String, bool) {
    let nullable = schema.schema_data.nullable;

    match &schema.schema_kind {
        SchemaKind::Type(Type::String(s)) => {
            let ty = match &s.format {
                VariantOrUnknownOrEmpty::Item(StringFormat::DateTime) => "DateTime<Utc>",
                VariantOrUnknownOrEmpty::Item(StringFormat::Date) => "NaiveDate",
                VariantOrUnknownOrEmpty::Unknown(f) if f == "uuid" => "Uuid",
                _ => "String",
            };
            (ty.to_string(), nullable)
        }
        SchemaKind::Type(Type::Integer(i)) => {
            let ty = match i.format {
                VariantOrUnknownOrEmpty::Item(IntegerFormat::Int32) => "i32",
                _ => "i64",
            };
            (ty.to_string(), nullable)
        }
        SchemaKind::Type(Type::Number(_)) => ("f64".to_string(), nullable),
        SchemaKind::Type(Type::Boolean(_)) => ("bool".to_string(), nullable),
        SchemaKind::Type(Type::Array(arr)) => {
            let inner = match &arr.items {
                Some(ref_or) => {
                    let (t, _) = resolve_field_type(ref_or, context, diagnostics);
                    t
                }
                None => {
                    record(
                        diagnostics,
                        Severity::Degraded,
                        context.to_string(),
                        "array without items",
                        "array has no `items` schema; element type is `serde_json::Value`",
                    );
                    "serde_json::Value".to_string()
                }
            };
            (format!("Vec<{inner}>"), nullable)
        }
        other => {
            record(
                diagnostics,
                Severity::Degraded,
                context.to_string(),
                describe_field_kind(other),
                "field type is not supported; generated as `serde_json::Value`",
            );
            ("serde_json::Value".to_string(), nullable)
        }
    }
}

/// A short label for an unsupported inline field schema kind, for diagnostics.
fn describe_field_kind(kind: &SchemaKind) -> &'static str {
    match kind {
        SchemaKind::Type(Type::Object(_)) => "inline object",
        SchemaKind::OneOf { .. } => "inline oneOf",
        SchemaKind::AllOf { .. } => "inline allOf",
        SchemaKind::AnyOf { .. } => "inline anyOf",
        SchemaKind::Not { .. } => "inline not",
        SchemaKind::Any(_) => "inline free-form schema",
        // The concrete scalar/array types are handled by `map_schema_to_type`;
        // this arm exists only for exhaustiveness.
        SchemaKind::Type(_) => "unsupported schema",
    }
}

/// Check whether a JSON default value can be represented as a Rust literal
/// for the given type. Inline enum fields pass `is_enum = true`.
fn is_supported_default(value: &serde_json::Value, rust_type: &str, is_enum: bool) -> bool {
    match value {
        serde_json::Value::String(_) => {
            matches!(rust_type, "String" | "DateTime<Utc>" | "NaiveDate" | "Uuid") || is_enum
        }
        serde_json::Value::Number(_) => matches!(rust_type, "i32" | "i64" | "f64"),
        serde_json::Value::Bool(_) => rust_type == "bool",
        _ => false,
    }
}

/// Extract and validate a default value. Returns `Some` if the default is
/// supported, `None` otherwise. Records a diagnostic when a default is present
/// in the spec but cannot be represented in the generated code.
fn extract_default(
    raw: &Option<serde_json::Value>,
    rust_type: &str,
    is_enum: bool,
    schema_name: &str,
    field_name: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<serde_json::Value> {
    let value = raw.as_ref()?;
    if is_supported_default(value, rust_type, is_enum) {
        Some(value.clone())
    } else {
        record(
            diagnostics,
            Severity::Degraded,
            format!("{schema_name}.{field_name}"),
            "default value",
            format!(
                "default value {value} ignored (type `{rust_type}` does not support code-generated defaults)"
            ),
        );
        None
    }
}

/// Convert a `PascalCase` string to `snake_case`.
pub(crate) fn to_snake_case(s: &str) -> String {
    let mut result = String::with_capacity(s.len() + 4);
    for (i, c) in s.chars().enumerate() {
        if c.is_uppercase() && i > 0 {
            result.push('_');
        }
        result.extend(c.to_lowercase());
    }
    result
}

/// Derive a PascalCase name from an HTTP method and path.
///
/// E.g. `("get", "/api/value-raw/{id}/timeseries")` → `"GetValueRawTimeseries"`.
/// Path parameters like `{id}` are stripped.
fn query_name_from_path(method: &str, path: &str) -> String {
    let segments = path
        .split('/')
        .filter(|s| !s.is_empty() && !s.starts_with('{'));
    let mut name = to_pascal_case(method);
    for segment in segments {
        name.push_str(&to_pascal_case(segment));
    }
    name
}

/// Convert a `snake_case` or `kebab-case` string to `PascalCase`.
pub(crate) fn to_pascal_case(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut capitalize_next = true;
    for c in s.chars() {
        if c == '_' || c == '-' {
            capitalize_next = true;
        } else if capitalize_next {
            result.extend(c.to_uppercase());
            capitalize_next = false;
        } else {
            result.push(c);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use openapiv3::OpenAPI;
    use pretty_assertions::assert_eq;

    use super::*;
    use crate::{Constraints, Entity, EntityKind, Field, StructDef, load_spec};

    type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

    const MINIMAL_HEADER: &str = r#"
openapi: "3.0.3"
info:
  title: Test
  version: "0.1.0"
"#;

    fn spec_with_schema(schema_yaml: &str) -> Result<OpenAPI> {
        let indented: String = schema_yaml
            .lines()
            .map(|line| {
                if line.trim().is_empty() {
                    String::new()
                } else {
                    format!("      {line}")
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        let full =
            format!("{MINIMAL_HEADER}paths: {{}}\ncomponents:\n  schemas:\n    Foo:\n{indented}\n");
        Ok(load_spec(&full)?)
    }

    fn first_struct_fields(entities: &[Entity]) -> &[Field] {
        match &entities[0] {
            Entity::Struct(s) => &s.fields,
            _ => panic!("expected Entity::Struct"),
        }
    }

    #[test]
    fn parse_schema_entity() -> Result<()> {
        let yaml = format!(
            r"{MINIMAL_HEADER}
paths: {{}}
components:
  schemas:
    Foo:
      type: object
      required: [id]
      properties:
        id:
          type: integer
          format: int64
        name:
          type: string
"
        );
        let entities = parse(&load_spec(&yaml)?);
        assert_eq!(entities.len(), 1);
        assert_eq!(
            entities[0],
            Entity::Struct(StructDef {
                name: String::from("Foo"),
                kind: EntityKind::Schema,
                fields: vec![
                    Field {
                        name: "id".into(),
                        rust_type: "i64".into(),
                        is_optional: false,
                        constraints: Constraints::None,
                        default_value: None,
                    },
                    Field {
                        name: "name".into(),
                        rust_type: "String".into(),
                        is_optional: true,
                        constraints: Constraints::None,
                        default_value: None,
                    },
                ],
                enums: vec![],
            })
        );

        Ok(())
    }

    #[test]
    fn parse_standalone_enum() -> Result<()> {
        let yaml = format!(
            r"{MINIMAL_HEADER}
paths: {{}}
components:
  schemas:
    Status:
      type: string
      enum: [ACTIVE, INACTIVE, PENDING]
"
        );
        let entities = parse(&load_spec(&yaml)?);
        assert_eq!(entities.len(), 1);
        assert_eq!(
            entities[0],
            Entity::Enum(EnumDef {
                name: "Status".into(),
                variants: vec![
                    ("ACTIVE".into(), "ACTIVE".into()),
                    ("INACTIVE".into(), "INACTIVE".into()),
                    ("PENDING".into(), "PENDING".into()),
                ],
            })
        );

        Ok(())
    }

    #[test]
    fn parse_query_entity() -> Result<()> {
        let yaml = format!(
            r#"{MINIMAL_HEADER}
paths:
  /things:
    get:
      operationId: getThings
      parameters:
        - name: limit
          in: query
          required: false
          schema:
            type: integer
            format: int32
        - name: id
          in: path
          required: true
          schema:
            type: integer
            format: int64
      responses:
        "200":
          description: OK
components:
  schemas: {{}}
"#
        );
        let entities = parse(&load_spec(&yaml)?);
        assert_eq!(entities.len(), 1);
        let Entity::Struct(s) = &entities[0] else {
            panic!("expected Entity::Struct");
        };
        assert_eq!(s.name, "GetThingsQuery");
        assert_eq!(s.kind, EntityKind::Query);
        assert_eq!(s.fields.len(), 1, "path param should be excluded");
        assert_eq!(s.fields[0].name, "limit");

        Ok(())
    }

    #[test]
    fn parse_query_without_operation_id() -> Result<()> {
        let yaml = format!(
            r#"{MINIMAL_HEADER}
paths:
  /api/value-raw/{{id}}/timeseries:
    get:
      parameters:
        - name: from
          in: query
          required: true
          schema:
            type: string
            format: date-time
      responses:
        "200":
          description: OK
components:
  schemas: {{}}
"#
        );
        let entities = parse(&load_spec(&yaml)?);
        assert_eq!(entities.len(), 1);
        let Entity::Struct(s) = &entities[0] else {
            panic!("expected Entity::Struct");
        };
        assert_eq!(s.name, "GetApiValueRawTimeseriesQuery");
        assert_eq!(s.kind, EntityKind::Query);
        assert_eq!(s.fields.len(), 1);
        assert_eq!(s.fields[0].name, "from");

        Ok(())
    }

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
            first_struct_fields(&parse(&spec))[0].constraints,
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
            first_struct_fields(&parse(&spec))[0].constraints,
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
            first_struct_fields(&parse(&spec))[0].constraints,
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
            first_struct_fields(&parse(&spec))[0].constraints,
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
            first_struct_fields(&parse(&spec))[0].constraints,
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
        assert_eq!(first_struct_fields(&parse(&spec))[0].rust_type, "bool");

        // Type mapping: number → f64
        let spec = spec_with_schema(
            "\
type: object
required: [score]
properties:
  score:
    type: number",
        )?;
        assert_eq!(first_struct_fields(&parse(&spec))[0].rust_type, "f64");

        Ok(())
    }

    /// Helper: build a spec with `Cat`/`Dog` object schemas plus a caller-supplied
    /// composite schema, and return the parsed entities.
    fn spec_with_composite(composite_yaml: &str) -> Result<Vec<Entity>> {
        let composite = composite_yaml
            .lines()
            .map(|line| {
                if line.trim().is_empty() {
                    String::new()
                } else {
                    format!("    {line}")
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        let full = format!(
            "{MINIMAL_HEADER}paths: {{}}\ncomponents:\n  schemas:\n    Cat:\n      type: object\n      properties:\n        name:\n          type: string\n    Dog:\n      type: object\n      properties:\n        name:\n          type: string\n{composite}\n"
        );
        Ok(parse(&load_spec(&full)?))
    }

    fn find_union<'a>(entities: &'a [Entity], name: &str) -> &'a UnionDef {
        entities
            .iter()
            .find_map(|e| match e {
                Entity::Union(u) if u.name == name => Some(u),
                _ => None,
            })
            .unwrap_or_else(|| panic!("expected Entity::Union named {name}"))
    }

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
        // No mapping → no variant is renamed on the wire.
        assert!(union.variants.iter().all(|v| v.wire_value.is_none()));
        assert_eq!(
            union
                .variants
                .iter()
                .map(|v| v.variant_name.as_str())
                .collect::<Vec<_>>(),
            vec!["Cat", "Dog"]
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

    /// Parse a full spec and return only the diagnostics.
    fn diagnostics_for(yaml: &str) -> Result<Vec<Diagnostic>> {
        let spec = load_spec(yaml)?;
        let mut diagnostics = Vec::new();
        let _ = parse_with_diagnostics(&spec, &mut diagnostics);
        Ok(diagnostics)
    }

    fn find_diag<'a>(diags: &'a [Diagnostic], construct: &str) -> &'a Diagnostic {
        diags
            .iter()
            .find(|d| d.construct == construct)
            .unwrap_or_else(|| panic!("expected a `{construct}` diagnostic in {diags:?}"))
    }

    #[test]
    fn diagnostic_all_of_dropped() -> Result<()> {
        let diags = diagnostics_for(&format!(
            r"{MINIMAL_HEADER}
paths: {{}}
components:
  schemas:
    Base:
      type: object
      properties:
        id:
          type: string
    Derived:
      allOf:
        - $ref: '#/components/schemas/Base'
"
        ))?;
        let d = find_diag(&diags, "allOf");
        assert_eq!(d.severity, Severity::Dropped);
        assert_eq!(d.path, "components.schemas.Derived");

        Ok(())
    }

    #[test]
    fn diagnostic_inline_object_field_degraded() -> Result<()> {
        let diags = diagnostics_for(&format!(
            r"{MINIMAL_HEADER}
paths: {{}}
components:
  schemas:
    Pet:
      type: object
      properties:
        metadata:
          type: object
          properties:
            key:
              type: string
"
        ))?;
        let d = find_diag(&diags, "inline object");
        assert_eq!(d.severity, Severity::Degraded);
        assert_eq!(d.path, "Pet.metadata");

        Ok(())
    }

    #[test]
    fn diagnostic_header_parameter_dropped() -> Result<()> {
        let diags = diagnostics_for(&format!(
            r#"{MINIMAL_HEADER}
paths:
  /things:
    get:
      operationId: getThings
      parameters:
        - name: X-Trace
          in: header
          required: false
          schema:
            type: string
      responses:
        "200":
          description: OK
components:
  schemas: {{}}
"#
        ))?;
        let d = find_diag(&diags, "header parameter");
        assert_eq!(d.severity, Severity::Dropped);
        assert_eq!(d.path, "GET /things#X-Trace");

        Ok(())
    }

    #[test]
    fn diagnostic_external_ref_degraded() -> Result<()> {
        let diags = diagnostics_for(&format!(
            r#"{MINIMAL_HEADER}
paths: {{}}
components:
  schemas:
    Order:
      type: object
      properties:
        customer:
          $ref: 'other.yaml#/components/schemas/Customer'
"#
        ))?;
        let d = find_diag(&diags, "external $ref");
        assert_eq!(d.severity, Severity::Degraded);
        assert_eq!(d.path, "Order.customer");

        Ok(())
    }

    #[test]
    fn diagnostic_additional_properties_degraded() -> Result<()> {
        let diags = diagnostics_for(&format!(
            r"{MINIMAL_HEADER}
paths: {{}}
components:
  schemas:
    Metadata:
      type: object
      additionalProperties:
        type: string
"
        ))?;
        let d = find_diag(&diags, "additionalProperties");
        assert_eq!(d.severity, Severity::Degraded);
        assert_eq!(d.path, "components.schemas.Metadata");

        Ok(())
    }

    /// A fully-supported spec must produce no diagnostics (guards against noise).
    #[test]
    fn diagnostic_clean_spec_is_silent() -> Result<()> {
        let diags = diagnostics_for(&format!(
            r"{MINIMAL_HEADER}
paths:
  /things:
    get:
      operationId: getThings
      parameters:
        - name: limit
          in: query
          required: false
          schema:
            type: integer
            format: int32
      responses:
        '200':
          description: OK
components:
  schemas:
    Thing:
      type: object
      required: [name]
      properties:
        name:
          type: string
          minLength: 1
        tags:
          type: array
          items:
            type: string
"
        ))?;
        assert!(diags.is_empty(), "expected no diagnostics, got {diags:?}");

        Ok(())
    }

    fn has_construct(diags: &[Diagnostic], construct: &str) -> bool {
        diags.iter().any(|d| d.construct == construct)
    }

    /// A `$ref` request body and a `$ref` response schema point at components we
    /// generate — no loss, no diagnostic. (Regression for the false-positive.)
    #[test]
    fn diagnostic_ref_bodies_are_clean() -> Result<()> {
        let diags = diagnostics_for(&format!(
            r##"{MINIMAL_HEADER}
paths:
  /things:
    post:
      operationId: createThing
      requestBody:
        content:
          application/json:
            schema:
              $ref: '#/components/schemas/Base'
      responses:
        '200':
          description: OK
          content:
            application/json:
              schema:
                $ref: '#/components/schemas/Base'
components:
  schemas:
    Base:
      type: object
      properties:
        id:
          type: string
"##
        ))?;
        assert!(diags.is_empty(), "expected no diagnostics, got {diags:?}");

        Ok(())
    }

    /// An inline request body schema has no generated type → diagnosed.
    #[test]
    fn diagnostic_inline_request_body() -> Result<()> {
        let diags = diagnostics_for(&format!(
            r#"{MINIMAL_HEADER}
paths:
  /things:
    post:
      operationId: createThing
      requestBody:
        content:
          application/json:
            schema:
              type: object
              properties:
                note:
                  type: string
      responses:
        "200":
          description: OK
components:
  schemas: {{}}
"#
        ))?;
        let d = find_diag(&diags, "request body");
        assert_eq!(d.severity, Severity::Dropped);
        assert_eq!(d.path, "POST /things");

        Ok(())
    }

    /// The `default` response is inspected too, not just numbered statuses.
    #[test]
    fn diagnostic_inline_default_response() -> Result<()> {
        let diags = diagnostics_for(&format!(
            r#"{MINIMAL_HEADER}
paths:
  /things:
    get:
      operationId: getThings
      responses:
        default:
          description: fallback
          content:
            application/json:
              schema:
                type: object
                properties:
                  code:
                    type: integer
components:
  schemas: {{}}
"#
        ))?;
        let d = find_diag(&diags, "response body");
        assert_eq!(d.severity, Severity::Dropped);
        assert_eq!(d.path, "GET /things");

        Ok(())
    }

    /// An unresolvable `$ref` request body is itself a genuine loss.
    #[test]
    fn diagnostic_unresolvable_ref_request_body() -> Result<()> {
        let diags = diagnostics_for(&format!(
            r##"{MINIMAL_HEADER}
paths:
  /things:
    post:
      operationId: createThing
      requestBody:
        $ref: '#/components/requestBodies/Missing'
      responses:
        "200":
          description: OK
components:
  schemas: {{}}
"##
        ))?;
        let d = find_diag(&diags, "request body");
        assert_eq!(d.severity, Severity::Dropped);
        assert!(
            d.reason.contains("could not resolve"),
            "reason: {}",
            d.reason
        );

        Ok(())
    }

    /// A `$ref` request body that resolves is judged by its content: inline
    /// content → diagnosed, all-`$ref` content → clean.
    #[test]
    fn diagnostic_resolvable_ref_request_body() -> Result<()> {
        // Resolves to a component whose content schema is inline → diagnosed.
        let inline = diagnostics_for(&format!(
            r##"{MINIMAL_HEADER}
paths:
  /things:
    post:
      operationId: createThing
      requestBody:
        $ref: '#/components/requestBodies/InlineBody'
      responses:
        "200":
          description: OK
components:
  schemas: {{}}
  requestBodies:
    InlineBody:
      content:
        application/json:
          schema:
            type: object
            properties:
              note:
                type: string
"##
        ))?;
        assert!(has_construct(&inline, "request body"));

        // Resolves to a component whose content schema is a $ref → clean.
        let refd = diagnostics_for(&format!(
            r##"{MINIMAL_HEADER}
paths:
  /things:
    post:
      operationId: createThing
      requestBody:
        $ref: '#/components/requestBodies/RefBody'
      responses:
        "200":
          description: OK
components:
  schemas:
    Base:
      type: object
      properties:
        id:
          type: string
  requestBodies:
    RefBody:
      content:
        application/json:
          schema:
            $ref: '#/components/schemas/Base'
"##
        ))?;
        assert!(
            !has_construct(&refd, "request body"),
            "resolved $ref-content body should be clean, got {refd:?}"
        );

        Ok(())
    }
}
