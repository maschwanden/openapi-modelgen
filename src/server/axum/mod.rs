//! Emission of the axum adapter module inside the generated `src/server.rs`.

use std::fmt::Write;

use super::{Route, RouteResponse, path_param_ident};
use crate::common::escape_keyword;

/// Whether a response renders at least one JSON body (needs the `Json` import).
fn response_has_json(response: &RouteResponse) -> bool {
    match response {
        RouteResponse::Empty => false,
        RouteResponse::Single { is_json, .. } => *is_json,
        RouteResponse::Negotiated { variants, .. } => variants.iter().any(|v| v.is_json),
    }
}

/// Emit the `#[cfg(feature = "server-axum")] mod axum_impl { .. }` block plus the
/// `pub use axum_impl::router;` re-export. Operates on the already-filtered
/// strict routes.
pub(crate) fn write_axum_module(
    out: &mut String,
    routes: &[&Route],
    any_secured: bool,
) -> std::fmt::Result {
    let needs_path = routes.iter().any(|r| !r.path_params.is_empty());
    let needs_query = routes.iter().any(|r| r.query_type.is_some());
    let needs_json = routes
        .iter()
        .any(|r| r.body_type.is_some() || response_has_json(&r.response));
    let needs_validation = routes
        .iter()
        .any(|r| r.query_type.is_some() || r.body_type.is_some());

    // Only the method that *starts* each path's chain is used as a free
    // `routing::<method>` function; subsequent methods are `MethodRouter`
    // methods (`.post(..)`), so importing them would be unused.
    let groups = group_routes_by_path(routes);
    let mut methods: Vec<&str> = groups
        .iter()
        .filter_map(|(_, g)| g.first().map(|r| r.method.as_str()))
        .collect();
    methods.sort_unstable();
    methods.dedup();

    // `Query` comes from `axum-extra` (repeated keys → `Vec`), not `axum`.
    let mut extractors = vec!["FromRequestParts", "State"];
    if needs_path {
        extractors.push("Path");
    }
    if needs_json {
        extractors.push("Json");
    }

    writeln!(out, "#[cfg(feature = \"server-axum\")]")?;
    writeln!(out, "mod axum_impl {{")?;
    writeln!(out, "    use super::Api;")?;
    if needs_validation {
        writeln!(out, "    use crate::Validation;")?;
    }
    writeln!(out, "    use axum::{{")?;
    writeln!(out, "        Router,")?;
    if !routes.is_empty() {
        writeln!(out, "        http::StatusCode,")?;
    }
    writeln!(out, "        extract::{{{}}},", extractors.join(", "))?;
    writeln!(out, "        response::IntoResponse,")?;
    writeln!(out, "        routing::{{{}}},", methods.join(", "))?;
    writeln!(out, "    }};")?;
    if needs_query {
        writeln!(out, "    use axum_extra::extract::Query;")?;
    }
    writeln!(out)?;

    // The free `router` fn wiring every generated route into an axum::Router.
    writeln!(
        out,
        "    /// Wire every generated route into an `axum::Router`. Add any manual\n\
         \x20   /// routes, then apply your state LAST:\n\
         \x20   /// `router::<MyApi>().route(\"/healthz\", ..).with_state(state)`."
    )?;
    writeln!(out, "    pub fn router<A>() -> Router<A::State>")?;
    writeln!(out, "    where")?;
    writeln!(out, "        A: Api + 'static,")?;
    writeln!(out, "        A::Error: IntoResponse,")?;
    writeln!(out, "        A::Ctx: FromRequestParts<A::State> + 'static,")?;
    if any_secured {
        writeln!(
            out,
            "        A::Auth: FromRequestParts<A::State> + 'static,"
        )?;
    }
    writeln!(out, "    {{")?;
    writeln!(out, "        Router::new()")?;
    for line in route_registration_lines(routes) {
        writeln!(out, "            {line}")?;
    }
    writeln!(out, "    }}")?;
    writeln!(out)?;

    for route in routes {
        write_handler_fn(out, route)?;
    }

    writeln!(out, "}}")?;
    writeln!(out)?;
    writeln!(out, "#[cfg(feature = \"server-axum\")]")?;
    writeln!(out, "pub use axum_impl::router;")?;
    Ok(())
}

/// Group routes by path template, preserving first-seen order.
fn group_routes_by_path<'a>(routes: &[&'a Route]) -> Vec<(String, Vec<&'a Route>)> {
    let mut groups: Vec<(String, Vec<&Route>)> = Vec::new();
    for route in routes {
        if let Some(g) = groups.iter_mut().find(|(p, _)| p == &route.path) {
            g.1.push(route);
        } else {
            groups.push((route.path.clone(), vec![route]));
        }
    }
    groups
}

/// Build `.route("<path>", get(..).post(..))` lines, grouping methods by path
/// in first-seen order.
fn route_registration_lines(routes: &[&Route]) -> Vec<String> {
    group_routes_by_path(routes)
        .into_iter()
        .map(|(path, group)| {
            let chained = group
                .iter()
                .map(|r| format!("{}(handle_{}::<A>)", r.method, r.handler))
                .collect::<Vec<_>>()
                .join(".");
            format!(".route(\"{path}\", {chained})")
        })
        .collect()
}

/// The `axum::http::StatusCode` expression for a success status code. Common
/// codes use the named constant; anything else falls back to `from_u16`.
fn status_code_expr(code: u16) -> String {
    match code {
        200 => "StatusCode::OK".to_string(),
        201 => "StatusCode::CREATED".to_string(),
        202 => "StatusCode::ACCEPTED".to_string(),
        204 => "StatusCode::NO_CONTENT".to_string(),
        other => format!("StatusCode::from_u16({other}).unwrap()"),
    }
}

/// Emit the private axum handler wrapper: extract → validate → call → wrap with
/// the spec's success status code.
fn write_handler_fn(out: &mut String, route: &Route) -> std::fmt::Result {
    let call = escape_keyword(&route.handler);
    let status = status_code_expr(route.success_status);
    // A single JSON body keeps a precise `(StatusCode, Json<T>)` type; raw and
    // negotiated bodies erase to `axum::response::Response`.
    let ret_ty = match &route.response {
        RouteResponse::Empty => "StatusCode".to_string(),
        RouteResponse::Single { is_json: true, .. } => {
            format!("(StatusCode, Json<{}>)", super::route_response_type(route))
        }
        RouteResponse::Single { is_json: false, .. } | RouteResponse::Negotiated { .. } => {
            "axum::response::Response".to_string()
        }
    };

    writeln!(
        out,
        "    async fn handle_{handler}<A>(",
        handler = route.handler
    )?;
    writeln!(out, "        State(state): State<A::State>,")?;
    writeln!(out, "        ctx: A::Ctx,")?;
    if route.secured {
        writeln!(out, "        auth: A::Auth,")?;
    }

    // Path extractor (positional), typed from the spec.
    let param_idents: Vec<String> = route
        .path_params
        .iter()
        .map(|(name, _)| path_param_ident(name))
        .collect();
    let param_types: Vec<&str> = route
        .path_params
        .iter()
        .map(|(_, ty)| ty.as_str())
        .collect();
    match param_idents.len() {
        0 => {}
        1 => writeln!(
            out,
            "        Path({}): Path<{}>,",
            param_idents[0], param_types[0]
        )?,
        _ => {
            let tuple = param_idents.join(", ");
            let types = param_types.join(", ");
            writeln!(out, "        Path(({tuple})): Path<({types})>,")?;
        }
    }
    if let Some(q) = &route.query_type {
        writeln!(out, "        Query(query): Query<crate::{q}>,")?;
    }
    if let Some(b) = &route.body_type {
        writeln!(out, "        Json(body): Json<crate::{b}>,")?;
    }
    writeln!(out, "    ) -> Result<{ret_ty}, A::Error>")?;
    writeln!(out, "    where")?;
    writeln!(out, "        A: Api + 'static,")?;
    writeln!(out, "        A::Error: IntoResponse,")?;
    writeln!(out, "        A::Ctx: FromRequestParts<A::State> + 'static,")?;
    if route.secured {
        writeln!(
            out,
            "        A::Auth: FromRequestParts<A::State> + 'static,"
        )?;
    }
    writeln!(out, "    {{")?;

    if route.query_type.is_some() {
        writeln!(out, "        query.validate()?;")?;
    }
    if route.body_type.is_some() {
        writeln!(out, "        body.validate()?;")?;
    }

    // Assemble the call arguments in the same order as the trait method.
    let mut call_args: Vec<String> = vec!["state".to_string(), "ctx".to_string()];
    if route.secured {
        call_args.push("auth".to_string());
    }
    call_args.extend(param_idents.clone());
    if route.query_type.is_some() {
        call_args.push("query".to_string());
    }
    if route.body_type.is_some() {
        call_args.push("body".to_string());
    }
    let args = call_args.join(", ");

    match &route.response {
        RouteResponse::Empty => {
            writeln!(out, "        A::{call}({args}).await?;")?;
            writeln!(out, "        Ok({status})")?;
        }
        RouteResponse::Single { is_json: true, .. } => {
            writeln!(out, "        let __resp = A::{call}({args}).await?;")?;
            writeln!(out, "        Ok(({status}, Json(__resp)))")?;
        }
        RouteResponse::Single {
            is_json: false,
            media_type,
            ..
        } => {
            writeln!(out, "        let __resp = A::{call}({args}).await?;")?;
            writeln!(
                out,
                "        Ok(({status}, [(axum::http::header::CONTENT_TYPE, \"{media_type}\")], __resp).into_response())"
            )?;
        }
        RouteResponse::Negotiated {
            enum_name,
            variants,
        } => {
            writeln!(out, "        let __resp = A::{call}({args}).await?;")?;
            writeln!(out, "        Ok(match __resp {{")?;
            for v in variants {
                if v.is_json {
                    writeln!(
                        out,
                        "            crate::{enum_name}::{variant}(__v) => ({status}, Json(__v)).into_response(),",
                        variant = v.variant,
                    )?;
                } else {
                    writeln!(
                        out,
                        "            crate::{enum_name}::{variant}(__v) => ({status}, [(axum::http::header::CONTENT_TYPE, \"{media}\")], __v).into_response(),",
                        variant = v.variant,
                        media = v.media_type,
                    )?;
                }
            }
            writeln!(out, "        }})")?;
        }
    }
    writeln!(out, "    }}")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::{Config, GeneratedCrate, Route, RouteRespVariant, RouteResponse, ServerStyle};

    type Result = std::result::Result<(), Box<dyn std::error::Error>>;

    /// A single JSON body response of `crate::{name}`.
    fn json_resp(name: &str) -> RouteResponse {
        RouteResponse::Single {
            rust_type: format!("crate::{name}"),
            media_type: "application/json".into(),
            is_json: true,
        }
    }

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

    fn base(method: &str, path: &str, handler: &str) -> Route {
        Route {
            method: method.into(),
            path: path.into(),
            handler: handler.into(),
            path_params: vec![],
            query_type: None,
            body_type: None,
            response: RouteResponse::Empty,
            success_status: 200,
            server_style: ServerStyle::Strict,
            secured: false,
        }
    }

    #[test]
    fn handler_honors_success_status_codes() -> Result {
        let routes = vec![
            // 201 Created, with a body.
            Route {
                body_type: Some("Thing".into()),
                response: json_resp("Thing"),
                success_status: 201,
                ..base("post", "/things", "create_thing")
            },
            // 204 No Content, no body.
            Route {
                success_status: 204,
                ..base("delete", "/things/{id}", "delete_thing")
            },
        ];
        let krate = crate::assemble(&[], &routes, &server_config())?;
        let server = find_file(&krate, "src/server.rs");
        assert!(
            server.contains("Ok((StatusCode::CREATED, Json(__resp)))"),
            "201 body response should carry CREATED: {server}"
        );
        assert!(
            server.contains("-> Result<(StatusCode, Json<crate::Thing>), A::Error>"),
            "body handler should return (StatusCode, Json<..>): {server}"
        );
        assert!(
            server.contains("Ok(StatusCode::NO_CONTENT)"),
            "204 empty response should return NO_CONTENT: {server}"
        );
        Ok(())
    }

    #[test]
    fn handler_content_negotiates() -> Result {
        let routes = vec![Route {
            response: RouteResponse::Negotiated {
                enum_name: "ListThingsResponse".into(),
                variants: vec![
                    RouteRespVariant {
                        variant: "Json".into(),
                        media_type: "application/json".into(),
                        is_json: true,
                    },
                    RouteRespVariant {
                        variant: "Csv".into(),
                        media_type: "text/csv".into(),
                        is_json: false,
                    },
                ],
            },
            ..base("get", "/things", "list_things")
        }];
        let krate = crate::assemble(&[], &routes, &server_config())?;
        let server = find_file(&krate, "src/server.rs");
        assert!(
            server.contains(
                "-> impl ::core::future::Future<Output = Result<crate::ListThingsResponse, Self::Error>> + Send;"
            ),
            "trait returns the response enum: {server}"
        );
        assert!(
            server
                .contains("crate::ListThingsResponse::Json(__v) => (StatusCode::OK, Json(__v)).into_response(),"),
            "JSON variant rendered via Json: {server}"
        );
        assert!(
            server.contains(
                "crate::ListThingsResponse::Csv(__v) => (StatusCode::OK, [(axum::http::header::CONTENT_TYPE, \"text/csv\")], __v).into_response(),"
            ),
            "CSV variant rendered with Content-Type: {server}"
        );
        Ok(())
    }

    #[test]
    fn router_wires_every_strict_route() -> Result {
        let routes = vec![
            Route {
                query_type: Some("ListThingsQuery".into()),
                response: json_resp("Thing"),
                ..base("get", "/things", "list_things")
            },
            Route {
                body_type: Some("Thing".into()),
                response: json_resp("Thing"),
                ..base("post", "/things", "create_thing")
            },
        ];
        let krate = crate::assemble(&[], &routes, &server_config())?;
        let server = find_file(&krate, "src/server.rs");
        assert!(
            server.contains(
                ".route(\"/things\", get(handle_list_things::<A>).post(handle_create_thing::<A>))"
            ),
            "routes not grouped/wired: {server}"
        );
        assert!(
            server.contains("pub fn router<A>() -> Router<A::State>"),
            "router fn: {server}"
        );
        assert!(
            server.contains("query.validate()?;") && server.contains("body.validate()?;"),
            "handlers should validate: {server}"
        );
        // Query extraction uses axum-extra so repeated keys collect into `Vec`.
        assert!(
            server.contains("use axum_extra::extract::Query;"),
            "query extractor should come from axum-extra: {server}"
        );
        let cargo = find_file(&krate, "Cargo.toml");
        assert!(
            cargo.contains("axum-extra = { version = \"0.10\", features = [\"query\"]")
                && cargo.contains("\"dep:axum-extra\""),
            "Cargo.toml should depend on axum-extra + gate it behind server-axum: {cargo}"
        );
        // `router` takes no state and returns `Router<A::State>`; the caller
        // applies `.with_state` after merging manual routes (so those manual
        // handlers can use `State<_>` too).
        assert!(
            !server.contains("pub fn router<A>(state:"),
            "router should not take/apply state itself: {server}"
        );
        Ok(())
    }

    #[test]
    fn secured_handler_extracts_auth() -> Result {
        let routes = vec![Route {
            body_type: Some("Thing".into()),
            response: json_resp("Thing"),
            secured: true,
            ..base("post", "/things", "create_thing")
        }];
        let krate = crate::assemble(&[], &routes, &server_config())?;
        let server = find_file(&krate, "src/server.rs");
        assert!(
            server.contains("auth: A::Auth,"),
            "auth extractor: {server}"
        );
        assert!(
            server.contains("A::create_thing(state, ctx, auth, body)"),
            "auth passed to trait method: {server}"
        );
        Ok(())
    }
}
