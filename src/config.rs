//! Parsing of the optional `openapi-modelgen.yaml` config file.

use std::collections::HashMap;

use serde::Deserialize;

use crate::ServerStyle;
use crate::error::Result;

/// The deserialized `openapi-modelgen.yaml`.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct FileConfig {
    /// Which artifacts to generate.
    #[serde(default)]
    pub generate: GenerateTargets,
    /// Project-wide default server style.
    #[serde(default)]
    pub server_style: Option<ServerStyle>,
    /// Per-operation overrides, keyed by `operationId`.
    #[serde(default)]
    pub operations: HashMap<String, OperationConfig>,
}

/// oapi-codegen-style boolean generation targets.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct GenerateTargets {
    /// Generate `model.rs` / `validation.rs` (+ `default.rs`). Default: true.
    #[serde(default = "default_true")]
    pub model: bool,
    /// Generate the framework-agnostic `Api` trait in `server.rs`. Default: false.
    #[serde(default)]
    pub server: bool,
    /// Generate the axum adapter module. Default: false. Implies `server`.
    #[serde(default)]
    pub axum_server: bool,
}

impl Default for GenerateTargets {
    fn default() -> Self {
        Self {
            model: true,
            server: false,
            axum_server: false,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct OperationConfig {
    #[serde(default)]
    pub server_style: Option<ServerStyle>,
}

fn default_true() -> bool {
    true
}

impl FileConfig {
    /// Parse a config from YAML text.
    pub fn parse(yaml: &str) -> Result<Self> {
        Ok(serde_yaml::from_str(yaml)?)
    }

    /// Validate the `axum-server ⇒ server ⇒ model` dependency rule.
    pub fn check_target_deps(&self) -> std::result::Result<(), String> {
        check_target_deps(
            self.generate.model,
            self.generate.server,
            self.generate.axum_server,
        )
    }

    /// The per-operation style overrides as a plain map.
    pub fn operation_styles(&self) -> HashMap<String, ServerStyle> {
        self.operations
            .iter()
            .filter_map(|(id, cfg)| cfg.server_style.map(|s| (id.clone(), s)))
            .collect()
    }
}

/// Validate the `axum-server ⇒ server ⇒ model` dependency rule.
pub fn check_target_deps(
    model: bool,
    server: bool,
    axum_server: bool,
) -> std::result::Result<(), String> {
    if axum_server && !server {
        return Err("generate.axum-server requires generate.server".to_string());
    }
    if server && !model {
        return Err("generate.server requires generate.model".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_full() {
        let yaml = "\
generate:
  model: true
  server: true
  axum-server: true
server-style: strict
operations:
  deleteThing:
    server-style: manual
";
        let cfg = FileConfig::parse(yaml).unwrap();
        assert!(cfg.generate.model && cfg.generate.server && cfg.generate.axum_server);
        assert_eq!(cfg.server_style, Some(ServerStyle::Strict));
        let styles = cfg.operation_styles();
        assert_eq!(styles.get("deleteThing"), Some(&ServerStyle::Manual));
        assert!(cfg.check_target_deps().is_ok());
    }

    #[test]
    fn defaults_when_absent() {
        let cfg = FileConfig::parse("{}").unwrap();
        assert!(cfg.generate.model);
        assert!(!cfg.generate.server);
        assert!(!cfg.generate.axum_server);
        assert_eq!(cfg.server_style, None);
        assert!(cfg.operations.is_empty());
    }

    #[test]
    fn dep_rule_violation() {
        assert!(check_target_deps(false, true, false).is_err());
        assert!(check_target_deps(true, false, true).is_err());
        assert!(check_target_deps(true, true, true).is_ok());
    }
}
