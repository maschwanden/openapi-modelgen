//! Server generation: parsing the routing table from a spec and emitting
//! `src/server.rs` (a framework-agnostic `Api` trait plus an opt-in axum adapter).

pub(crate) mod axum;
pub(crate) mod parse;

use std::fmt::Write;

use crate::ServerStyle;
use crate::common::{escape_keyword, header_comment, to_pascal_case, to_snake_case};

/// A single HTTP operation extracted from an OpenAPI path item.
///
/// Only populated when server generation is requested. Types referenced here
/// (`query_type`, `body_type`, `response_type`) are names of structs generated
/// into `model.rs` and re-exported at the crate root.
#[derive(Debug, PartialEq)]
pub struct Route {
    /// Lowercase HTTP method: `get` / `put` / `post` / `patch` / `delete`.
    pub method: String,
    /// OpenAPI path template, e.g. `/things/{id}`.
    pub path: String,
    /// snake_case handler name (from `operationId`, else derived from method+path).
    pub handler: String,
    /// Path parameter names in URL order. All treated as `String` in this prototype.
    pub path_params: Vec<String>,
    /// Generated query-parameter struct name, if the operation has query params.
    pub query_type: Option<String>,
    /// Request-body struct name (a `$ref` schema), if a JSON body is present.
    pub body_type: Option<String>,
    /// Success (200/201) response struct name (a `$ref` schema), if any.
    pub response_type: Option<String>,
    /// Whether the success response is a `Vec` of `response_type` (array schema).
    pub response_array: bool,
    /// How this operation is served: in the generated trait (`Strict`) or
    /// hand-wired by the user (`Manual`, excluded from generated code).
    pub server_style: ServerStyle,
    /// Whether an OpenAPI `security` requirement is in effect for this operation.
    /// When true, the generated method receives an `auth: Self::Auth` argument.
    pub secured: bool,
}

/// Generate `src/server.rs`: a framework-agnostic `Api` trait plus an opt-in
/// axum adapter (emitted only when `emit_axum`).
///
/// Only `Strict` routes are emitted; `Manual` routes are dropped entirely and
/// listed in a doc comment so they remain discoverable.
pub(crate) fn server_rs(routes: &[Route], emit_axum: bool) -> Result<String, std::fmt::Error> {
    let strict: Vec<&Route> = routes
        .iter()
        .filter(|r| r.server_style == ServerStyle::Strict)
        .collect();
    let manual: Vec<&Route> = routes
        .iter()
        .filter(|r| r.server_style == ServerStyle::Manual)
        .collect();
    let any_secured = strict.iter().any(|r| r.secured);

    let header = header_comment();
    let mut out = String::new();

    writeln!(out, "// {header}")?;
    writeln!(out)?;
    writeln!(
        out,
        "//! Server scaffolding generated from the OpenAPI paths.\n\
         //!\n\
         //! Implement [`Api`] for your application type — the compiler will list\n\
         //! any operation you have not handled yet. With the `server-axum` feature,\n\
         //! [`router`] wires every generated operation into an `axum::Router`."
    )?;
    if !manual.is_empty() {
        writeln!(out, "//!")?;
        writeln!(
            out,
            "//! Operations configured as `manual` are NOT part of this trait — wire them\n\
             //! yourself and merge them into `router(..)`:"
        )?;
        for r in &manual {
            writeln!(out, "//!   - `{} {}`", r.method.to_uppercase(), r.path)?;
        }
    }
    writeln!(out)?;

    // --- framework-agnostic core trait -------------------------------------
    writeln!(
        out,
        "/// One method per OpenAPI operation. No default bodies: a missing route\n\
         /// is a `not all trait items implemented` compile error.\n\
         ///\n\
         /// `State`, `Error` and `Ctx` (a general request extractor) are yours to\n\
         /// choose. Secured operations additionally receive `Auth`."
    )?;
    writeln!(out, "pub trait Api {{")?;
    writeln!(out, "    type State: Clone + Send + Sync + 'static;")?;
    writeln!(out, "    type Error: From<crate::ValidationError> + Send;")?;
    writeln!(out, "    type Ctx: Send;")?;
    if any_secured {
        writeln!(out, "    type Auth: Send;")?;
    }
    for route in &strict {
        write_trait_method(&mut out, route)?;
    }
    writeln!(out, "}}")?;
    writeln!(out)?;

    // --- axum adapter ------------------------------------------------------
    if emit_axum {
        axum::write_axum_module(&mut out, &strict, any_secured)?;
    }

    Ok(out)
}

/// The success/response type for a route as a Rust type string.
pub(crate) fn route_response_type(route: &Route) -> String {
    match &route.response_type {
        Some(t) if route.response_array => format!("Vec<crate::{t}>"),
        Some(t) => format!("crate::{t}"),
        None => "()".to_string(),
    }
}

/// snake_case + keyword-escaped identifier for a path parameter.
pub(crate) fn path_param_ident(raw: &str) -> String {
    escape_keyword(&to_snake_case(&to_pascal_case(raw)))
}

/// Emit the trait method signature (RPITIT with an explicit `Send` bound so the
/// returned future is usable directly as an axum handler).
fn write_trait_method(out: &mut String, route: &Route) -> std::fmt::Result {
    let name = escape_keyword(&route.handler);
    let mut args = String::from("state: Self::State, ctx: Self::Ctx");
    if route.secured {
        write!(args, ", auth: Self::Auth")?;
    }
    for p in &route.path_params {
        write!(args, ", {}: String", path_param_ident(p))?;
    }
    if let Some(q) = &route.query_type {
        write!(args, ", query: crate::{q}")?;
    }
    if let Some(b) = &route.body_type {
        write!(args, ", body: crate::{b}")?;
    }
    let resp = route_response_type(route);
    writeln!(
        out,
        "    /// `{method} {path}`",
        method = route.method.to_uppercase(),
        path = route.path
    )?;
    writeln!(
        out,
        "    fn {name}({args}) \
         -> impl ::core::future::Future<Output = Result<{resp}, Self::Error>> + Send;"
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Config, GeneratedCrate, ServerStyle};

    type Result = std::result::Result<(), Box<dyn std::error::Error>>;

    fn server_config() -> Config {
        let mut c = Config::new("test_api".to_string(), false);
        c.emit_server = true;
        c.emit_axum = true;
        c
    }

    fn find_file<'a>(krate: &'a GeneratedCrate, path: &str) -> &'a str {
        &krate
            .files
            .iter()
            .find(|f| f.path == path)
            .unwrap_or_else(|| panic!("missing file: {path}"))
            .content
    }

    fn route(method: &str, path: &str, handler: &str) -> Route {
        Route {
            method: method.into(),
            path: path.into(),
            handler: handler.into(),
            path_params: vec![],
            query_type: None,
            body_type: None,
            response_type: None,
            response_array: false,
            server_style: ServerStyle::Strict,
            secured: false,
        }
    }

    fn demo_routes() -> Vec<Route> {
        vec![
            Route {
                query_type: Some("ListThingsQuery".into()),
                response_type: Some("Thing".into()),
                ..route("get", "/things", "list_things")
            },
            Route {
                body_type: Some("Thing".into()),
                response_type: Some("Thing".into()),
                ..route("post", "/things", "create_thing")
            },
            Route {
                path_params: vec!["id".into()],
                response_type: Some("Thing".into()),
                ..route("get", "/things/{id}", "get_thing_by_id")
            },
        ]
    }

    #[test]
    fn strict_trait_uses_associated_types() -> Result {
        let krate = crate::assemble(&[], &demo_routes(), &server_config())?;
        let server = find_file(&krate, "src/server.rs");

        assert!(server.contains("pub trait Api {"), "trait header: {server}");
        assert!(
            server.contains("type State: Clone + Send + Sync + 'static;")
                && server.contains("type Error: From<crate::ValidationError> + Send;")
                && server.contains("type Ctx: Send;"),
            "associated types: {server}"
        );
        assert!(
            server.contains(
                "fn list_things(state: Self::State, ctx: Self::Ctx, query: crate::ListThingsQuery) \
                 -> impl ::core::future::Future<Output = Result<crate::Thing, Self::Error>> + Send;"
            ),
            "list_things signature: {server}"
        );
        assert!(
            server.contains("fn get_thing_by_id(state: Self::State, ctx: Self::Ctx, id: String)"),
            "path-param signature: {server}"
        );
        // No security in this spec → no Auth machinery.
        assert!(!server.contains("type Auth"), "unexpected Auth: {server}");
        // The axum entry point is the free `router` fn, not an extension trait.
        assert!(
            server.contains("pub fn router<A>() -> Router<A::State>")
                && !server.contains("ApiAxumExt"),
            "router entry point: {server}"
        );
        // No generated ApiError any more.
        assert!(
            !server.contains("pub struct ApiError"),
            "ApiError should be gone: {server}"
        );
        Ok(())
    }

    #[test]
    fn manual_operation_is_excluded() -> Result {
        let mut routes = demo_routes();
        // Mark get_thing_by_id manual.
        routes[2].server_style = ServerStyle::Manual;
        let krate = crate::assemble(&[], &routes, &server_config())?;
        let server = find_file(&krate, "src/server.rs");

        assert!(
            !server.contains("fn get_thing_by_id"),
            "manual op should be excluded from the trait: {server}"
        );
        assert!(
            !server.contains(".route(\"/things/{id}\""),
            "manual op should not be wired: {server}"
        );
        assert!(
            server.contains("`GET /things/{id}`"),
            "manual op should be documented: {server}"
        );
        Ok(())
    }

    #[test]
    fn security_adds_auth() -> Result {
        let mut routes = demo_routes();
        routes[1].secured = true; // create_thing requires auth
        let krate = crate::assemble(&[], &routes, &server_config())?;
        let server = find_file(&krate, "src/server.rs");

        assert!(server.contains("type Auth: Send;"), "Auth type: {server}");
        assert!(
            server.contains(
                "fn create_thing(state: Self::State, ctx: Self::Ctx, auth: Self::Auth, body: crate::Thing)"
            ),
            "secured op gets auth param: {server}"
        );
        // Public op does NOT get auth.
        assert!(
            server.contains("fn list_things(state: Self::State, ctx: Self::Ctx, query:"),
            "public op unchanged: {server}"
        );
        Ok(())
    }

    #[test]
    fn array_response_becomes_vec() -> Result {
        let routes = vec![Route {
            response_type: Some("Thing".into()),
            response_array: true,
            ..route("get", "/things", "list_things")
        }];
        let krate = crate::assemble(&[], &routes, &server_config())?;
        let server = find_file(&krate, "src/server.rs");
        assert!(
            server.contains(
                "-> impl ::core::future::Future<Output = Result<Vec<crate::Thing>, Self::Error>> + Send;"
            ),
            "array response should be Vec in trait: {server}"
        );
        assert!(
            server.contains("Json<Vec<crate::Thing>>"),
            "handler should wrap Json<Vec<..>>: {server}"
        );
        Ok(())
    }
}
