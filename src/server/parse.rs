use openapiv3::{
    IntegerFormat, OpenAPI, Operation, ReferenceOr, Schema, SchemaKind, Type,
    VariantOrUnknownOrEmpty,
};

use super::{Route, RouteRespVariant, RouteResponse};
use crate::common::{
    operation_base_name, query_name_from_path, resolve_parameter_ref, resolve_ref_name,
    success_response_media, to_pascal_case, to_snake_case,
};
use crate::diagnostics::{Severity, record};
use crate::{Config, Diagnostic, ServerStyle};

/// Parse the routing table (one [`Route`] per HTTP operation) from a spec.
///
/// This is intentionally separate from model parsing: models are always
/// generated, but routes are only needed when server generation is requested.
/// Per-operation `server-style` and `security` are resolved here.
pub(crate) fn parse_routes(
    spec: &OpenAPI,
    config: &Config,
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<Route> {
    let global_secured = spec.security.as_ref().is_some_and(|reqs| !reqs.is_empty());
    let mut routes = Vec::new();

    for (path, path_item_ref) in spec.paths.iter() {
        let path_item = match path_item_ref {
            ReferenceOr::Item(item) => item,
            ReferenceOr::Reference { .. } => continue,
        };

        let ops: [(&str, &Option<Operation>); 5] = [
            ("get", &path_item.get),
            ("put", &path_item.put),
            ("post", &path_item.post),
            ("patch", &path_item.patch),
            ("delete", &path_item.delete),
        ];
        for (method, op) in ops {
            if let Some(op) = op {
                let style = effective_style(op, config);
                let secured = is_secured(op, global_secured);
                routes.push(build_route(
                    op,
                    spec.components.as_ref(),
                    method,
                    path,
                    style,
                    secured,
                ));
            }
        }
    }

    diagnose_duplicate_handlers(&routes, diagnostics);
    routes
}

/// Record a diagnostic for any trait-method name shared by more than one
/// `strict` route. Duplicate names would be an `E0428` in the generated trait,
/// so this surfaces the collision — almost always a spec with missing or
/// duplicate operationIds — as a diagnostic instead of cryptic non-compiling
/// output. (With distinct path segments the derived names no longer collide;
/// this catches duplicate operationIds and pathological path shapes.)
fn diagnose_duplicate_handlers(routes: &[Route], diagnostics: &mut Vec<Diagnostic>) {
    use std::collections::{HashMap, HashSet};

    let mut counts: HashMap<&str, usize> = HashMap::new();
    for r in routes
        .iter()
        .filter(|r| r.server_style == ServerStyle::Strict)
    {
        *counts.entry(r.handler.as_str()).or_default() += 1;
    }
    let mut reported: HashSet<&str> = HashSet::new();
    for r in routes
        .iter()
        .filter(|r| r.server_style == ServerStyle::Strict)
    {
        let handler = r.handler.as_str();
        if counts[handler] > 1 && reported.insert(handler) {
            record(
                diagnostics,
                Severity::Dropped,
                format!("Api::{handler}"),
                "duplicate operation name",
                format!(
                    "{n} operations map to the trait method `{handler}`; give them distinct \
                     operationIds (the generated trait would not compile otherwise)",
                    n = counts[handler],
                ),
            );
        }
    }
}

/// Resolve the effective `server-style` for an operation: a per-operation
/// override (keyed by `operationId`) falls back to the project default.
fn effective_style(op: &Operation, config: &Config) -> ServerStyle {
    op.operation_id
        .as_ref()
        .and_then(|id| config.operation_styles.get(id).copied())
        .unwrap_or(config.server_style)
}

/// Whether an OpenAPI `security` requirement is in effect for this operation.
///
/// An operation-level `security` overrides the global one; an explicit empty
/// array (`security: []`) makes the operation public even if a global
/// requirement exists.
fn is_secured(op: &Operation, global_secured: bool) -> bool {
    match &op.security {
        Some(reqs) => !reqs.is_empty(),
        None => global_secured,
    }
}

fn build_route(
    op: &Operation,
    components: Option<&openapiv3::Components>,
    method: &str,
    path: &str,
    server_style: ServerStyle,
    secured: bool,
) -> Route {
    // Normalise via PascalCase → snake_case so camelCase / kebab-case /
    // snake_case operationIds all yield a valid snake_case identifier.
    let handler = to_snake_case(&operation_base_name(op, method, path));

    let body_type = op.request_body.as_ref().and_then(|rb| match rb {
        ReferenceOr::Item(rb) => json_schema_ref_name(rb.content.get("application/json")),
        ReferenceOr::Reference { .. } => None,
    });

    let (success_status, media) = success_response_media(&op.responses);
    let response = match media.as_slice() {
        [] => RouteResponse::Empty,
        [only] => RouteResponse::Single {
            rust_type: only.crate_type.clone(),
            media_type: only.media_type.clone(),
            is_json: only.is_json,
        },
        _ => RouteResponse::Negotiated {
            enum_name: format!("{}Response", operation_base_name(op, method, path)),
            variants: media
                .iter()
                .map(|m| RouteRespVariant {
                    variant: m.variant.clone(),
                    media_type: m.media_type.clone(),
                    is_json: m.is_json,
                })
                .collect(),
        },
    };

    Route {
        method: method.to_string(),
        path: path.to_string(),
        handler,
        path_params: path_params(op, components, path),
        query_type: query_struct_name(op, components, method, path),
        body_type,
        response,
        success_status,
        server_style,
        secured,
    }
}

/// Extract `{name}` path-parameter names from a path template, in URL order.
fn path_param_names(path: &str) -> Vec<String> {
    path.split('/')
        .filter_map(|seg| {
            seg.strip_prefix('{')
                .and_then(|s| s.strip_suffix('}'))
                .map(str::to_string)
        })
        .collect()
}

/// Path parameters as `(name, rust_type)` pairs, in URL order. The type is
/// resolved from the operation's matching `in: path` parameter schema so the
/// generated methods receive e.g. `i64` instead of `String`.
fn path_params(
    op: &Operation,
    components: Option<&openapiv3::Components>,
    path: &str,
) -> Vec<(String, String)> {
    path_param_names(path)
        .into_iter()
        .map(|name| {
            let ty = path_param_type(op, components, &name).unwrap_or_else(|| "String".to_string());
            (name, ty)
        })
        .collect()
}

/// Resolve the Rust type of a single `in: path` parameter by `name`, looking
/// through `$ref` parameters. Returns `None` when the parameter or its schema
/// is missing (caller falls back to `String`).
fn path_param_type(
    op: &Operation,
    components: Option<&openapiv3::Components>,
    name: &str,
) -> Option<String> {
    op.parameters.iter().find_map(|p| {
        let param = match p {
            ReferenceOr::Item(param) => Some(param),
            ReferenceOr::Reference { reference } => resolve_parameter_ref(reference, components),
        };
        match param {
            Some(openapiv3::Parameter::Path { parameter_data, .. })
                if parameter_data.name == name =>
            {
                match &parameter_data.format {
                    openapiv3::ParameterSchemaOrContent::Schema(schema) => {
                        Some(scalar_param_type(schema))
                    }
                    openapiv3::ParameterSchemaOrContent::Content(_) => None,
                }
            }
            _ => None,
        }
    })
}

/// Map a (required, non-nullable) path-parameter schema to a Rust scalar type.
/// Only plain scalars are supported; anything else (incl. `uuid`/`date`
/// formats and non-scalar shapes) falls back to `String`.
fn scalar_param_type(schema_ref: &ReferenceOr<Schema>) -> String {
    let ReferenceOr::Item(schema) = schema_ref else {
        return "String".to_string();
    };
    match &schema.schema_kind {
        SchemaKind::Type(Type::Integer(i)) => match i.format {
            VariantOrUnknownOrEmpty::Item(IntegerFormat::Int32) => "i32",
            _ => "i64",
        }
        .to_string(),
        SchemaKind::Type(Type::Number(_)) => "f64".to_string(),
        SchemaKind::Type(Type::Boolean(_)) => "bool".to_string(),
        _ => "String".to_string(),
    }
}

/// The generated query-struct name for an operation, if it has query params.
/// Mirrors the naming used by model query parsing.
fn query_struct_name(
    op: &Operation,
    components: Option<&openapiv3::Components>,
    method: &str,
    path: &str,
) -> Option<String> {
    let has_query = op.parameters.iter().any(|p| {
        let resolved = match p {
            ReferenceOr::Item(param) => Some(param),
            ReferenceOr::Reference { reference } => resolve_parameter_ref(reference, components),
        };
        matches!(resolved, Some(openapiv3::Parameter::Query { .. }))
    });
    if !has_query {
        return None;
    }
    Some(match &op.operation_id {
        Some(id) => format!("{}Query", to_pascal_case(id)),
        None => format!("{}Query", query_name_from_path(method, path)),
    })
}

/// Given the `application/json` media type (if present), return the referenced
/// schema name. Inline (non-`$ref`) bodies are unsupported in this prototype.
fn json_schema_ref_name(media: Option<&openapiv3::MediaType>) -> Option<String> {
    match media?.schema.as_ref()? {
        ReferenceOr::Reference { reference } => Some(resolve_ref_name(reference)),
        ReferenceOr::Item(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Config, ServerStyle, load_spec};

    type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

    const HEADER: &str = "openapi: \"3.0.3\"\ninfo:\n  title: T\n  version: \"0.1.0\"\n";

    fn find<'a>(routes: &'a [Route], method: &str, path: &str) -> &'a Route {
        routes
            .iter()
            .find(|r| r.method == method && r.path == path)
            .unwrap_or_else(|| panic!("route {method} {path} not found"))
    }

    #[test]
    fn per_op_style_override() -> Result<()> {
        let yaml = format!(
            "{HEADER}paths:\n  /things:\n    get:\n      operationId: listThings\n      responses:\n        \"200\":\n          description: ok\n  /things/{{id}}:\n    delete:\n      operationId: deleteThing\n      parameters:\n        - name: id\n          in: path\n          required: true\n          schema:\n            type: string\n      responses:\n        \"204\":\n          description: ok\n"
        );
        let spec = load_spec(&yaml)?;
        let mut config = Config::new("x".into(), false);
        config
            .operation_styles
            .insert("deleteThing".into(), ServerStyle::Manual);
        let routes = parse_routes(&spec, &config, &mut Vec::new());
        assert_eq!(
            find(&routes, "get", "/things").server_style,
            ServerStyle::Strict
        );
        assert_eq!(
            find(&routes, "delete", "/things/{id}").server_style,
            ServerStyle::Manual
        );
        Ok(())
    }

    #[test]
    fn security_resolution() -> Result<()> {
        // Global security requirement, one op opts out with `security: []`.
        let yaml = format!(
            "{HEADER}security:\n  - apiKey: []\npaths:\n  /a:\n    get:\n      operationId: a\n      responses:\n        \"200\":\n          description: ok\n  /b:\n    get:\n      operationId: b\n      security: []\n      responses:\n        \"200\":\n          description: ok\n  /c:\n    get:\n      operationId: c\n      security:\n        - apiKey: []\n      responses:\n        \"200\":\n          description: ok\n"
        );
        let spec = load_spec(&yaml)?;
        let routes = parse_routes(&spec, &Config::new("x".into(), false), &mut Vec::new());
        assert!(find(&routes, "get", "/a").secured, "inherits global");
        assert!(!find(&routes, "get", "/b").secured, "security: [] opts out");
        assert!(find(&routes, "get", "/c").secured, "explicit op security");
        Ok(())
    }

    #[test]
    fn no_security_means_public() -> Result<()> {
        let yaml = format!(
            "{HEADER}paths:\n  /a:\n    get:\n      operationId: a\n      responses:\n        \"200\":\n          description: ok\n"
        );
        let spec = load_spec(&yaml)?;
        let routes = parse_routes(&spec, &Config::new("x".into(), false), &mut Vec::new());
        assert!(!find(&routes, "get", "/a").secured);
        Ok(())
    }

    #[test]
    fn array_response_detected() -> Result<()> {
        let yaml = format!(
            "{HEADER}paths:\n  /things:\n    get:\n      operationId: listThings\n      responses:\n        \"200\":\n          description: ok\n          content:\n            application/json:\n              schema:\n                type: array\n                items:\n                  $ref: \"#/components/schemas/Thing\"\ncomponents:\n  schemas:\n    Thing:\n      type: object\n      properties:\n        id:\n          type: string\n"
        );
        let spec = load_spec(&yaml)?;
        let routes = parse_routes(&spec, &Config::new("x".into(), false), &mut Vec::new());
        let r = find(&routes, "get", "/things");
        assert_eq!(
            r.response,
            RouteResponse::Single {
                rust_type: "Vec<crate::Thing>".into(),
                media_type: "application/json".into(),
                is_json: true,
            }
        );
        Ok(())
    }

    #[test]
    fn path_param_type_from_spec() -> Result<()> {
        let yaml = format!(
            "{HEADER}paths:\n  /things/{{thing_id}}:\n    get:\n      operationId: getThing\n      parameters:\n        - name: thing_id\n          in: path\n          required: true\n          schema:\n            type: integer\n            format: int64\n      responses:\n        \"200\":\n          description: ok\n"
        );
        let spec = load_spec(&yaml)?;
        let routes = parse_routes(&spec, &Config::new("x".into(), false), &mut Vec::new());
        let r = find(&routes, "get", "/things/{thing_id}");
        assert_eq!(
            r.path_params,
            vec![("thing_id".to_string(), "i64".to_string())]
        );
        Ok(())
    }

    #[test]
    fn sibling_paths_get_unique_names_without_operation_ids() -> Result<()> {
        // No operationIds: `/things` and `/things/{id}` used to both derive
        // `get_things` (path param stripped) — a duplicate trait method.
        let yaml = format!(
            "{HEADER}paths:\n  /things:\n    get:\n      responses:\n        \"200\":\n          description: ok\n  /things/{{id}}:\n    get:\n      parameters:\n        - name: id\n          in: path\n          required: true\n          schema:\n            type: string\n      responses:\n        \"200\":\n          description: ok\n"
        );
        let spec = load_spec(&yaml)?;
        let mut diags = Vec::new();
        let routes = parse_routes(&spec, &Config::new("x".into(), false), &mut diags);
        assert_eq!(find(&routes, "get", "/things").handler, "get_things");
        assert_eq!(
            find(&routes, "get", "/things/{id}").handler,
            "get_things_by_id"
        );
        assert!(diags.is_empty(), "unique names → no diagnostic: {diags:?}");
        Ok(())
    }

    #[test]
    fn duplicate_operation_ids_are_diagnosed() -> Result<()> {
        let yaml = format!(
            "{HEADER}paths:\n  /a:\n    get:\n      operationId: sameName\n      responses:\n        \"200\":\n          description: ok\n  /b:\n    get:\n      operationId: sameName\n      responses:\n        \"200\":\n          description: ok\n"
        );
        let spec = load_spec(&yaml)?;
        let mut diags = Vec::new();
        let _ = parse_routes(&spec, &Config::new("x".into(), false), &mut diags);
        assert_eq!(
            diags
                .iter()
                .filter(|d| d.construct == "duplicate operation name")
                .count(),
            1,
            "one diagnostic per colliding name: {diags:?}"
        );
        Ok(())
    }
}
