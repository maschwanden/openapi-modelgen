//! End-to-end collision behavior: two spec names that map to one Rust name
//! abort the run. Nothing is written: a half-generated crate whose names do
//! not match the spec is worse than no crate at all.

use std::{fs, process::Command};

struct Run {
    status: std::process::ExitStatus,
    stderr: String,
    out_dir: std::path::PathBuf,
}

fn run(spec: &str, dir_name: &str) -> Run {
    let base = std::env::temp_dir().join(dir_name);
    let _ = fs::remove_dir_all(&base);
    let out = base.join("out");
    fs::create_dir_all(&out).unwrap();
    let spec_path = base.join("spec.yaml");
    fs::write(&spec_path, spec).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_openapi-modelgen"))
        .args([
            "--input",
            spec_path.to_str().unwrap(),
            "--output-dir",
            out.to_str().unwrap(),
            "--crate-name",
            "collision_test",
        ])
        .output()
        .unwrap();

    Run {
        status: output.status,
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        out_dir: out.join("collision_test"),
    }
}

/// Two enum values that map to one variant: the run fails, says which values
/// clashed, and leaves no crate behind.
#[test]
fn colliding_enum_values_fail_the_run() {
    let spec = r#"
openapi: "3.0.3"
info:
  title: Test
  version: "0.1.0"
paths: {}
components:
  schemas:
    Series:
      type: object
      properties:
        resolution:
          type: string
          enum: ["10min", "Variant10min"]
"#;
    let run = run(spec, "openapi_modelgen_collision_enum");

    assert!(!run.status.success());
    assert_eq!(run.status.code(), Some(1));
    assert!(
        run.stderr.contains(
            r#"enum values "10min" and "Variant10min" would both become the Rust enum variant `Variant10min`"#
        ),
        "stderr should name both values: {}",
        run.stderr
    );
    assert!(
        !run.out_dir.exists(),
        "no crate should have been written to {}",
        run.out_dir.display()
    );
}

/// A spec whose names all resolve is unaffected: it still generates.
#[test]
fn distinct_names_still_generate() {
    let spec = r#"
openapi: "3.0.3"
info:
  title: Test
  version: "0.1.0"
paths: {}
components:
  schemas:
    Series:
      type: object
      properties:
        resolution:
          type: string
          enum: ["10min", "1h", "hourly"]
"#;
    let run = run(spec, "openapi_modelgen_collision_clean");

    assert!(run.status.success(), "stderr: {}", run.stderr);
    assert!(run.out_dir.join("src/model.rs").exists());
}
