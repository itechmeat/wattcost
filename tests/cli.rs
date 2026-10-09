//! Runs the real binary the way the panel extension and the install instructions do.

use std::io::Write;
use std::process::{Command, Output, Stdio};

use tempfile::TempDir;

/// A wattcost process with its own config and data directories.
struct Wattcost {
    home: TempDir,
}

impl Wattcost {
    fn new() -> Self {
        Self {
            home: TempDir::new().expect("temporary directory"),
        }
    }

    fn run(&self, args: &[&str], stdin: &str) -> Output {
        let mut child = Command::new(env!("CARGO_BIN_EXE_wattcost"))
            .args(args)
            .env("XDG_CONFIG_HOME", self.home.path().join("config"))
            .env("XDG_DATA_HOME", self.home.path().join("data"))
            .env("NO_COLOR", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start wattcost");
        child
            .stdin
            .take()
            .expect("stdin")
            .write_all(stdin.as_bytes())
            .expect("write stdin");
        child.wait_with_output().expect("wait for wattcost")
    }

    fn json(&self, args: &[&str]) -> serde_json::Value {
        let output = self.run(args, "");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).expect("JSON on stdout")
    }
}

const TARIFF: &str = r#"{"currency":"USD","day":{"start_hour":6,"end_hour":22,"price":0.31},"night":{"start_hour":22,"end_hour":6,"price":0.12}}"#;

#[test]
fn config_set_then_show_round_trips() {
    let wattcost = Wattcost::new();
    let output = wattcost.run(&["config", "set"], TARIFF);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let shown = wattcost.json(&["config", "show"]);
    assert_eq!(
        shown,
        serde_json::from_str::<serde_json::Value>(TARIFF).unwrap()
    );
}

#[test]
fn config_set_reports_problems_without_saving() {
    let wattcost = Wattcost::new();
    let overlap = TARIFF.replace(r#""start_hour":22"#, r#""start_hour":21"#);
    for (input, expected) in [
        ("not json", "reading the tariff JSON"),
        (overlap.as_str(), "overlap at 21:00"),
    ] {
        let output = wattcost.run(&["config", "set"], input);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!output.status.success());
        assert!(
            stderr.starts_with("error: ") && stderr.contains(expected),
            "{stderr}"
        );
    }
    assert!(
        !wattcost
            .home
            .path()
            .join("config/wattcost/config.toml")
            .exists()
    );
}

#[test]
fn series_works_before_any_data_exists() {
    let series = Wattcost::new().json(&["series", "--span", "week"]);
    assert_eq!(series["currency"], "EUR");
    // Hourly points over seven local days; a daylight-saving change adds or removes one.
    assert!((7 * 24 - 1..=7 * 24 + 1).contains(&series["points"].as_array().unwrap().len()));
    assert_eq!(series["today_cost"], 0.0);
}

#[test]
fn report_rejects_an_end_date_without_a_start() {
    let output = Wattcost::new().run(&["report", "--to", "2026-01-31"], "");
    assert_eq!(output.status.code(), Some(2));
}
