#![allow(clippy::doc_markdown)]

use std::{
    collections::HashMap,
    env, fs,
    path::{Path, PathBuf},
    process,
};

use clap::Parser;

use openapi_modelgen::{
    Config, FileConfig, ServerStyle, Severity, check_target_deps, generate, load_spec,
};

#[derive(Parser)]
#[command(version)]
struct Args {
    /// Path to the OpenAPI YAML spec
    #[arg(long)]
    input: PathBuf,

    /// Parent directory for the generated crate (a subdirectory named after
    /// `--crate-name` will be created inside it)
    #[arg(long)]
    output_dir: PathBuf,

    /// Cargo package name (e.g., `argos-openapi`)
    #[arg(long)]
    crate_name: String,

    /// Use workspace dependency references (e.g., `chrono.workspace = true`).
    /// If not set, uses fixed version numbers.
    #[arg(long, default_value_t = false)]
    workspace: bool,

    /// Fail (non-zero exit) if any part of the spec was dropped (a construct
    /// that produced no output). Degraded-only runs still succeed. Files are
    /// always written; the exit code lets CI gate on lossless generation.
    #[arg(long, default_value_t = false)]
    strict: bool,

    /// Also generate `src/server.rs` with a framework-agnostic `Api` trait and
    /// an opt-in axum adapter. Shorthand for `generate: { server, axum-server }`
    /// in the config file; overrides it when both are given.
    #[arg(long, default_value_t = false)]
    server: bool,

    /// Optional `openapi-modelgen.yaml` config file selecting generation targets
    /// (`generate: { model, server, axum-server }`) and `server-style`.
    #[arg(long)]
    config: Option<PathBuf>,
}

fn main() {
    env_logger::Builder::from_default_env()
        .filter_level(log::LevelFilter::Warn)
        .init();

    let args = Args::parse();
    let input = expand_tilde(&args.input);
    let output_dir = expand_tilde(&args.output_dir);

    if !input.exists() {
        eprintln!("error: input file '{}' does not exist", input.display());
        process::exit(1);
    }
    if !input.is_file() {
        eprintln!("error: input path '{}' is not a file", input.display());
        process::exit(1);
    }

    if !output_dir.exists() {
        eprintln!(
            "error: output directory '{}' does not exist",
            output_dir.display()
        );
        process::exit(1);
    }
    if !output_dir.is_dir() {
        eprintln!(
            "error: output path '{}' is not a directory",
            output_dir.display()
        );
        process::exit(1);
    }

    if !is_valid_crate_name(&args.crate_name) {
        eprintln!(
            "error: '{}' is not a valid crate name (must start with a letter, digit, or \
             underscore and contain only alphanumeric characters, hyphens, or underscores)",
            args.crate_name
        );
        process::exit(1);
    }

    let crate_dir = output_dir.join(&args.crate_name);

    // Generation targets: config file (if any), then the `--server` CLI shorthand.
    let mut emit_model = true;
    let mut emit_server = false;
    let mut emit_axum = false;
    let mut server_style = ServerStyle::Strict;
    let mut operation_styles: HashMap<String, ServerStyle> = HashMap::new();

    if let Some(cfg_path) = &args.config {
        let cfg_path = expand_tilde(cfg_path);
        let text = fs::read_to_string(&cfg_path).unwrap_or_else(|e| {
            eprintln!("error: failed to read config '{}': {e}", cfg_path.display());
            process::exit(1);
        });
        let file_config = FileConfig::parse(&text).unwrap_or_else(|e| {
            eprintln!(
                "error: failed to parse config '{}': {e}",
                cfg_path.display()
            );
            process::exit(1);
        });
        emit_model = file_config.generate.model;
        emit_server = file_config.generate.server;
        emit_axum = file_config.generate.axum_server;
        if let Some(style) = file_config.server_style {
            server_style = style;
        }
        operation_styles = file_config.operation_styles();
    }

    // `--server` is a shorthand for enabling both server targets, overriding config.
    if args.server {
        emit_server = true;
        emit_axum = true;
    }

    if let Err(e) = check_target_deps(emit_model, emit_server, emit_axum) {
        eprintln!("error: invalid generation targets: {e}");
        process::exit(1);
    }

    if operation_styles.is_empty() && server_style == ServerStyle::Strict && !emit_server {
        // No server config in effect and no server requested — nothing to warn about.
    } else if !emit_server && !emit_axum {
        eprintln!(
            "warning: server-style/operations in the config are ignored because neither \
             `server` nor `axum-server` generation is enabled"
        );
    }

    let config = Config {
        crate_name: args.crate_name,
        use_workspace: args.workspace,
        emit_model,
        emit_server,
        emit_axum,
        server_style,
        operation_styles,
    };

    let content = fs::read_to_string(&input).unwrap_or_else(|e| {
        eprintln!("error: failed to read '{}': {e}", input.display());
        process::exit(1);
    });
    let spec = load_spec(&content).unwrap_or_else(|e| {
        eprintln!("error: failed to parse OpenAPI spec: {e}");
        process::exit(1);
    });
    let generated = generate(&spec, &config).unwrap_or_else(|e| {
        eprintln!("error: code generation failed: {e}");
        process::exit(1);
    });

    report_diagnostics(&generated.diagnostics);

    for file in &generated.files {
        let path = crate_dir.join(file.path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .unwrap_or_else(|e| panic!("failed to create {}: {e}", parent.display()));
        }
        fs::write(&path, &file.content)
            .unwrap_or_else(|e| panic!("failed to write {}: {e}", path.display()));
    }

    if args.strict {
        let dropped = generated
            .diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Dropped)
            .count();
        if dropped > 0 {
            eprintln!("error: --strict: {dropped} spec construct(s) were dropped");
            process::exit(2);
        }
    }
}

/// Print a summary of anything the generator could not fully represent.
fn report_diagnostics(diagnostics: &[openapi_modelgen::Diagnostic]) {
    if diagnostics.is_empty() {
        return;
    }
    let dropped = diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Dropped)
        .count();
    let degraded = diagnostics.len() - dropped;
    eprintln!(
        "warning: {} spec construct(s) were not fully generated ({dropped} dropped, {degraded} degraded):",
        diagnostics.len()
    );
    for diagnostic in diagnostics {
        eprintln!("  - {diagnostic}");
    }
}

fn expand_tilde(path: &Path) -> PathBuf {
    if let Ok(stripped) = path.strip_prefix("~")
        && let Some(home) = env::var_os("HOME")
    {
        return PathBuf::from(home).join(stripped);
    }
    path.to_path_buf()
}

fn is_valid_crate_name(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    let first = name.as_bytes()[0];
    if !(first.is_ascii_alphanumeric() || first == b'_') {
        return false;
    }
    name.bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}
