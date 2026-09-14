//! Who logs a diagnostic, and who does not.
//!
//! Recording a diagnostic never logs. `generate` returns the list instead, so
//! that a caller (the CLI) can render its own summary without each diagnostic
//! being printed twice. The `parse` convenience wrapper is the exception: it
//! discards the list, so it surfaces every diagnostic through `log` at `warn`
//! rather than letting them vanish.
//!
//! One test drives both paths, since the logger and the capture buffer are
//! process-global and tests share a process.

use std::sync::Mutex;

use log::{Level, Metadata, Record};
use openapi_modelgen::{Config, generate, load_spec, parse};

static LOGS: Mutex<Vec<String>> = Mutex::new(Vec::new());

struct CaptureLogger;

impl log::Log for CaptureLogger {
    fn enabled(&self, meta: &Metadata) -> bool {
        meta.level() <= Level::Warn
    }
    fn log(&self, record: &Record) {
        if record.level() <= Level::Warn {
            LOGS.lock().unwrap().push(record.args().to_string());
        }
    }
    fn flush(&self) {}
}

const LOSSY_SPEC: &str = r##"
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

#[test]
fn logging_contract() {
    log::set_logger(&CaptureLogger).unwrap();
    log::set_max_level(log::LevelFilter::Warn);

    let spec = load_spec(LOSSY_SPEC).unwrap();
    let config = Config {
        crate_name: "log_test".to_string(),
        use_workspace: true,
    };

    // generate() returns diagnostics and must NOT log (the CLI prints its own
    // summary from the returned list; logging too would double-print).
    LOGS.lock().unwrap().clear();
    let generated = generate(&spec, &config).unwrap();
    assert!(
        !generated.diagnostics.is_empty(),
        "spec should produce diagnostics"
    );
    assert!(
        LOGS.lock().unwrap().is_empty(),
        "generate() must not log, captured: {:?}",
        LOGS.lock().unwrap()
    );

    // parse() discards the list, so it surfaces each diagnostic at warn.
    LOGS.lock().unwrap().clear();
    let _ = parse(&spec);
    assert!(
        !LOGS.lock().unwrap().is_empty(),
        "parse() should log dropped/degraded diagnostics at warn"
    );
}
