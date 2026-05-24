#![allow(clippy::doc_markdown)]

use std::{
    env, fs,
    path::{Path, PathBuf},
    process,
};

use clap::Parser;

use openapi_modelgen::{Config, generate, load_spec};

#[derive(Parser)]
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
}

fn main() {
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

    let config = Config {
        crate_name: args.crate_name,
        use_workspace: args.workspace,
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

    for file in &generated.files {
        let path = crate_dir.join(file.path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .unwrap_or_else(|e| panic!("failed to create {}: {e}", parent.display()));
        }
        fs::write(&path, &file.content)
            .unwrap_or_else(|e| panic!("failed to write {}: {e}", path.display()));
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
