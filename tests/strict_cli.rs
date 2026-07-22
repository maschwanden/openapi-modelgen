//! End-to-end `--strict` behavior: it fails only on *dropped* constructs.

use std::{fs, process::Command};

fn run_strict(spec: &str, dir_name: &str) -> std::process::ExitStatus {
    let base = std::env::temp_dir().join(dir_name);
    let _ = fs::remove_dir_all(&base);
    let out = base.join("out");
    fs::create_dir_all(&out).unwrap();
    let spec_path = base.join("spec.yaml");
    fs::write(&spec_path, spec).unwrap();

    Command::new(env!("CARGO_BIN_EXE_openapi-modelgen"))
        .args([
            "--input",
            spec_path.to_str().unwrap(),
            "--output-dir",
            out.to_str().unwrap(),
            "--crate-name",
            "strict_test",
            "--strict",
        ])
        .status()
        .unwrap()
}

/// A dropped construct (`allOf`) makes `--strict` exit non-zero.
#[test]
fn strict_fails_on_dropped() {
    let spec = r##"
openapi: "3.0.3"
info:
  title: Test
  version: "0.1.0"
paths: {}
components:
  schemas:
    Base:
      type: object
      properties:
        id:
          type: string
    Derived:
      allOf:
        - $ref: "#/components/schemas/Base"
"##;
    let status = run_strict(spec, "openapi_modelgen_strict_dropped");
    assert!(!status.success());
    assert_eq!(status.code(), Some(2));
}

/// A degraded-only spec (inline object field, nothing dropped) exits 0 even
/// under `--strict`.
#[test]
fn strict_passes_on_degraded_only() {
    let spec = r#"
openapi: "3.0.3"
info:
  title: Test
  version: "0.1.0"
paths: {}
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
"#;
    let status = run_strict(spec, "openapi_modelgen_strict_degraded");
    assert!(
        status.success(),
        "degraded-only should exit 0, got {status:?}"
    );
}
