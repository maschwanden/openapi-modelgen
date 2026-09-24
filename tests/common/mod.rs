//! Helpers shared by the end-to-end generation tests. Each integration test
//! binary compiles this module separately, so not every helper is used in all
//! of them.
#![allow(dead_code)]

use openapi_modelgen::{Config, Error, GeneratedCrate, generate, load_spec};

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

pub fn test_config() -> Config {
    Config {
        crate_name: "test_api".to_string(),
        use_workspace: true,
    }
}

/// Generate, expecting a fatal problem, and return the rendered error.
pub fn spec_error(yaml: &str) -> String {
    let spec = load_spec(yaml).expect("spec should parse");
    let Err(err) = generate(&spec, &test_config()) else {
        panic!("generation should have been aborted");
    };
    assert!(
        matches!(err, Error::Unrepresentable(_)),
        "expected a name collision, got {err}"
    );
    err.to_string()
}

pub fn file_content<'a>(crate_: &'a GeneratedCrate, path: &str) -> &'a str {
    &crate_
        .files
        .iter()
        .find(|f| f.path == path)
        .unwrap_or_else(|| panic!("missing file: {path}"))
        .content
}
