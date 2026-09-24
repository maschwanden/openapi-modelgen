//! Everything reached through `paths`: query parameters and operation bodies.

use openapiv3::{Operation, ReferenceOr};

use super::{
    constraint::extract_constraints, default::extract_default, schema::resolve_schema_ref,
};
use crate::{
    Constraints, Diagnostic, Entity, EntityKind, Field, StructDef,
    diagnostic::{Severity, record},
    ident::{to_pascal_case, to_type_ident},
};

pub(super) fn parse_query(
    op: &Operation,
    components: Option<&openapiv3::Components>,
    method: &str,
    path: &str,
) -> (Option<Entity>, Vec<Diagnostic>) {
    let location = format!("{} {path}", method.to_uppercase());

    let mut diagnostics = Vec::new();
    let mut query_params = Vec::new();
    for p in &op.parameters {
        let param = match p {
            ReferenceOr::Item(param) => param,
            ReferenceOr::Reference { reference } => {
                match resolve_parameter_ref(reference, components) {
                    Some(param) => param,
                    None => {
                        record(
                            &mut diagnostics,
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
            // Path parameters live in the URL, not the query struct: excluded by design.
            openapiv3::Parameter::Path { .. } => {}
            openapiv3::Parameter::Header { parameter_data, .. } => record(
                &mut diagnostics,
                Severity::Dropped,
                format!("{location}#{}", parameter_data.name),
                "header parameter",
                "header parameters are not generated",
            ),
            openapiv3::Parameter::Cookie { parameter_data, .. } => record(
                &mut diagnostics,
                Severity::Dropped,
                format!("{location}#{}", parameter_data.name),
                "cookie parameter",
                "cookie parameters are not generated",
            ),
        }
    }

    // The parameters already reported are a loss whether or not a struct comes
    // out of this operation, so they leave with the `None` too.
    if query_params.is_empty() {
        return (None, diagnostics);
    }

    // An operationId that yields no name falls back to the path, which always
    // starts with the HTTP method and so always yields one.
    let struct_name = op
        .operation_id
        .as_deref()
        .and_then(to_type_ident)
        .unwrap_or_else(|| query_name_from_path(method, path))
        + "Query";

    let mut fields = Vec::new();

    for param in &query_params {
        let data = parameter_data(param);
        let param_path = format!("{location}#{}", data.name);
        let openapiv3::ParameterSchemaOrContent::Schema(schema_ref) = &data.format else {
            record(
                &mut diagnostics,
                Severity::Dropped,
                param_path,
                "content parameter",
                "parameter uses `content` instead of `schema`; not generated",
            );
            continue;
        };
        let (rust_type, nullable, reported) = resolve_schema_ref(schema_ref, &param_path);
        diagnostics.extend(reported);

        let default_value = match schema_ref {
            ReferenceOr::Item(schema) => {
                let (value, reported) =
                    extract_default(&schema.schema_data.default, &rust_type, None, &param_path);
                diagnostics.extend(reported);
                value
            }
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
            ref_target: match schema_ref {
                ReferenceOr::Reference { reference } => Some(reference.clone()),
                ReferenceOr::Item(_) => None,
            },
            is_optional,
            constraints,
            default_value,
            // Query parameters never generate inline enums.
            is_inline_enum: false,
        });
    }

    (
        Some(Entity::Struct(StructDef {
            name: struct_name,
            kind: EntityKind::Query,
            fields,
            enums: Vec::new(),
        })),
        diagnostics,
    )
}

/// Report operation request/response bodies, which are never parsed into types.
///
/// A body is a loss only when its content schema is *inline*; a `$ref` content
/// schema points at a component we do generate. `$ref` bodies/responses are
/// resolved against `components`, then the same inline check applies: an
/// unresolvable `$ref` (external or missing) is itself a genuine loss.
pub(super) fn diagnose_operation_bodies(
    op: &Operation,
    components: Option<&openapiv3::Components>,
    method: &str,
    path: &str,
) -> Vec<Diagnostic> {
    let location = format!("{} {path}", method.to_uppercase());
    let mut diagnostics = Vec::new();

    // Request body: a loss only when its content schema is inline. A `$ref`
    // body is resolved first; an unresolvable `$ref` is itself a loss.
    if let Some(ref_or) = &op.request_body {
        let resolved = match ref_or {
            ReferenceOr::Item(body) => Some(body),
            ReferenceOr::Reference { reference } => {
                let body = resolve_request_body(reference, components);
                if body.is_none() {
                    record(
                        &mut diagnostics,
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
                &mut diagnostics,
                Severity::Dropped,
                location.clone(),
                "request body",
                "inline request body schema is not generated as a named type",
            );
        }
    }

    // Responses (every status plus the `default` response). Each status is
    // reported separately so the diagnostic names the response that was lost.
    let by_status = op
        .responses
        .responses
        .iter()
        .map(|(status, response_ref)| (status.to_string(), response_ref))
        .chain(
            op.responses
                .default
                .iter()
                .map(|response_ref| ("default".to_string(), response_ref)),
        );
    for (status, response_ref) in by_status {
        let response_path = format!("{location}#{status}");
        let resolved = match response_ref {
            ReferenceOr::Item(response) => Some(response),
            ReferenceOr::Reference { reference } => {
                let response = resolve_response(reference, components);
                if response.is_none() {
                    record(
                        &mut diagnostics,
                        Severity::Dropped,
                        response_path.clone(),
                        "response body",
                        format!("could not resolve response $ref `{reference}`"),
                    );
                }
                response
            }
        };
        if let Some(response) = resolved
            && content_has_inline_schema(response.content.values())
        {
            record(
                &mut diagnostics,
                Severity::Dropped,
                response_path,
                "response body",
                "inline response body schema is not generated as a named type",
            );
        }
    }

    diagnostics
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

/// Derive a PascalCase name from an HTTP method and path. Always a valid
/// identifier: the HTTP method opens it.
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

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use crate::{Entity, EntityKind, diagnostic::Severity, load_spec, parse, parse::testutil::*};

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
        let (entities, _) = parse(&load_spec(&yaml)?);
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
        let (entities, _) = parse(&load_spec(&yaml)?);
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

    /// A `$ref` request body and a `$ref` response schema point at components we
    /// generate: no loss, no diagnostic. Reporting one here would blame the
    /// operation for a type that was generated after all, and `--strict` would
    /// then fail a spec that lost nothing.
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
        assert_eq!(d.path, "GET /things#default");

        Ok(())
    }

    /// Every lossy response is reported separately, named by its status, rather
    /// than collapsed into one diagnostic for the whole operation.
    #[test]
    fn diagnostic_inline_response_per_status() -> Result<()> {
        let diags = diagnostics_for(&format!(
            r#"{MINIMAL_HEADER}
paths:
  /things:
    get:
      operationId: getThings
      responses:
        "200":
          description: OK
          content:
            application/json:
              schema:
                type: object
                properties:
                  a:
                    type: string
        "404":
          description: nope
          content:
            application/json:
              schema:
                type: object
                properties:
                  b:
                    type: string
components:
  schemas: {{}}
"#
        ))?;
        let paths: Vec<&str> = diags
            .iter()
            .filter(|d| d.construct == "response body")
            .map(|d| d.path.as_str())
            .collect();
        assert_eq!(paths, vec!["GET /things#200", "GET /things#404"]);

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
