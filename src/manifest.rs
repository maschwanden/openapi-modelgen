//! Assemble the generated crate's `Cargo.toml` and `src/lib.rs` from the model
//! and server generation needs.

use crate::Config;
use crate::common::header_comment;

pub(crate) fn generate_cargo_toml(
    config: &Config,
    needs_regex: bool,
    needs_uuid: bool,
    needs_axum_query: bool,
) -> String {
    let header = header_comment();
    // Server deps use fixed versions in both modes: a consuming workspace is
    // unlikely to pin axum/tokio, and these are opt-in extras anyway.
    // `axum-extra` provides the query extractor that collects repeated keys
    // (`?k=a&k=b`) into `Vec`, which plain `axum::extract::Query` cannot.
    let server_deps = {
        let mut s = String::new();
        if config.emit_axum {
            s.push_str("axum = { version = \"0.8\", optional = true }\n");
            s.push_str("tokio = { version = \"1\", features = [\"full\"], optional = true }\n");
            if needs_axum_query {
                s.push_str(
                    "axum-extra = { version = \"0.10\", features = [\"query\"], optional = true }\n",
                );
            }
        }
        s
    };
    let server_features = {
        let mut s = String::new();
        if config.emit_server || config.emit_axum {
            s.push_str("\n[features]\n");
        }
        if config.emit_server {
            s.push_str(
                "# Enables the framework-agnostic `Api` trait in `server.rs`.\nserver = []\n",
            );
        }
        if config.emit_axum {
            let extra_dep = if needs_axum_query {
                ", \"dep:axum-extra\""
            } else {
                ""
            };
            s.push_str(&format!(
                "# Enables the axum adapter (`server::router`).\n\
                 server-axum = [\"dep:axum\", \"dep:tokio\"{extra_dep}, \"server\"]\n"
            ));
        }
        s
    };
    if config.use_workspace {
        let regex_dep = if needs_regex {
            "regex.workspace = true\n"
        } else {
            ""
        };
        let uuid_dep = if needs_uuid {
            "uuid = { workspace = true, features = [\"serde\"] }\n"
        } else {
            ""
        };
        format!(
            "\
# {header}

[package]
name = \"{crate_name}\"
version = \"0.1.0\"
edition = \"2024\"

[dependencies]
chrono = {{ workspace = true, features = [\"serde\"] }}
{regex_dep}serde = {{ workspace = true, features = [\"derive\"] }}
serde_json.workspace = true
{uuid_dep}{server_deps}
[dev-dependencies]
pretty_assertions.workspace = true
{server_features}",
            crate_name = config.crate_name,
        )
    } else {
        let regex_dep = if needs_regex { "regex = \"1\"\n" } else { "" };
        let uuid_dep = if needs_uuid {
            "uuid = { version = \"1\", features = [\"serde\"] }\n"
        } else {
            ""
        };
        format!(
            "\
# {header}

[package]
name = \"{crate_name}\"
version = \"0.1.0\"
edition = \"2024\"

[dependencies]
chrono = {{ version = \"0.4\", features = [\"serde\"] }}
{regex_dep}serde = {{ version = \"1\", features = [\"derive\"] }}
serde_json = \"1\"
{uuid_dep}{server_deps}
[dev-dependencies]
pretty_assertions = \"1\"
{server_features}",
            crate_name = config.crate_name,
        )
    }
}

pub(crate) fn generate_lib_rs(config: &Config, needs_defaults: bool) -> String {
    let header = header_comment();
    let default_mod = if needs_defaults { "mod default;\n" } else { "" };
    let server_mod = if config.emit_server {
        "\n#[cfg(feature = \"server\")]\npub mod server;\n"
    } else {
        ""
    };
    format!(
        "\
// {header}

{default_mod}mod model;
mod validation;

pub use model::*;
pub use validation::{{Validation, ValidationError}};
{server_mod}"
    )
}
