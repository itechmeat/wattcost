//! Runs the collector with the metrics endpoint on an ephemeral port against a prepared database.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};

use chrono::{DateTime, TimeZone, Utc};
use tempfile::TempDir;
use wattcost::pricing::Pricing;
use wattcost::providers::Quality;
use wattcost::sample::{ComponentStats, RawSample};
use wattcost::store::Store;

// One sample per minute and no sensors: the first collected row would appear only a minute or
// more after start, so the served values come from the prepared rows alone.
const CONFIG: &str = r#"
[collector]
sample_interval_s = 60
write_interval_s = 60
database = "DATABASE"

[hardware]
base_watts = 30.0
psu_efficiency = 0.9
monitor_watts = 40.0
providers = { rapl = false, cpu_stat = false, cpu_estimate = false, nvml = false, amdgpu = false, battery = false, display = false }

[tariff]
currency = "USD"
tax_multipliers = [1.5]

[[tariff.periods]]
name = "flat"
start = "00:00"
end = "00:00"
price = 0.2
"#;

fn start() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 1, 15, 12, 0, 0).unwrap()
}

fn row(minute: i64, cpu_j: f64, gpu_j: Option<f64>, wall_wh: f64) -> (RawSample, Pricing) {
    let sample = RawSample {
        start: start() + chrono::TimeDelta::minutes(minute),
        duration_s: 60.0,
        cpu: ComponentStats {
            energy_j: Some(cpu_j),
            quality: Quality::Measured,
            ..ComponentStats::default()
        },
        gpu: ComponentStats {
            energy_j: gpu_j,
            quality: if gpu_j.is_some() {
                Quality::Measured
            } else {
                Quality::Missing
            },
            ..ComponentStats::default()
        },
        display_on_s: Some(30.0),
        ..RawSample::default()
    };
    let pricing = Pricing {
        wall_wh,
        period: "flat".to_owned(),
        price_per_kwh: 0.3,
        cost: wall_wh / 1000.0 * 0.3,
        currency: "USD".to_owned(),
    };
    (sample, pricing)
}

/// A `wattcost run` serving metrics, stopped when dropped.
struct Collector {
    child: Child,
    address: String,
    _home: TempDir,
}

impl Collector {
    fn start() -> Self {
        let home = TempDir::new().expect("temporary directory");
        let database = home.path().join("wattcost.db");
        let store = Store::open(&database).expect("open the database");
        let settings_id = store
            .settings_id("prepared", start())
            .expect("settings row");
        for (sample, pricing) in [row(0, 1200.0, Some(600.0), 1.0), row(1, 2400.0, None, 2.0)] {
            store
                .insert_sample(&sample, &pricing, settings_id)
                .expect("insert a row");
        }
        let config = home.path().join("config.toml");
        std::fs::write(
            &config,
            CONFIG.replace("DATABASE", database.to_str().expect("UTF-8 path")),
        )
        .expect("write the config");
        let mut child = Command::new(env!("CARGO_BIN_EXE_wattcost"))
            .arg("--config")
            .arg(&config)
            .args(["run", "--metrics-listen", "127.0.0.1:0"])
            .env("NO_COLOR", "1")
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start wattcost");
        let mut lines = BufReader::new(child.stderr.take().expect("stderr")).lines();
        let address = lines.by_ref().map_while(Result::ok).find_map(|line| {
            let (_, rest) = line.split_once("serving metrics on http://")?;
            rest.strip_suffix("/metrics").map(str::to_owned)
        });
        // Logging into a closed pipe would stop the collector.
        std::thread::spawn(move || lines.for_each(drop));
        Self {
            address: address.expect("the metrics address in the log"),
            child,
            _home: home,
        }
    }

    fn request(&self, head: &str) -> String {
        let mut stream = TcpStream::connect(&self.address).expect("connect");
        stream.write_all(head.as_bytes()).expect("send the request");
        let mut response = String::new();
        stream
            .read_to_string(&mut response)
            .expect("read the response");
        response
    }
}

impl Drop for Collector {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn value(body: &str, series: &str) -> f64 {
    let line = body
        .lines()
        .find(|line| line.split_once(' ').is_some_and(|(name, _)| name == series))
        .unwrap_or_else(|| panic!("no {series} in\n{body}"));
    line.rsplit_once(' ').unwrap().1.parse().expect("a number")
}

fn assert_close(actual: f64, expected: f64) {
    assert!((actual - expected).abs() < 1e-9, "{actual} != {expected}");
}

#[test]
fn serves_totals_and_the_last_row_from_the_database() {
    let collector = Collector::start();
    let response = collector.request("GET /metrics HTTP/1.1\r\nHost: test\r\n\r\n");
    let (head, body) = response.split_once("\r\n\r\n").expect("headers and body");
    assert!(head.starts_with("HTTP/1.1 200 OK\r\n"), "{head}");
    assert!(
        head.contains("Content-Type: text/plain; version=0.0.4; charset=utf-8"),
        "{head}"
    );

    assert!(body.contains("# TYPE wattcost_energy_joules_total counter\n"));
    assert!(body.contains("# TYPE wattcost_power_watts gauge\n"));
    let version = format!(
        "wattcost_build_info{{version=\"{}\"}}",
        env!("CARGO_PKG_VERSION")
    );
    assert_close(value(body, &version), 1.0);
    assert_close(
        value(body, "wattcost_energy_joules_total{component=\"cpu\"}"),
        3600.0,
    );
    assert_close(
        value(body, "wattcost_energy_joules_total{component=\"gpu\"}"),
        600.0,
    );
    assert_close(
        value(body, "wattcost_wall_energy_joules_total"),
        3.0 * 3600.0,
    );
    assert_close(value(body, "wattcost_cost_total{currency=\"USD\"}"), 0.0009);
    assert_close(value(body, "wattcost_power_watts{component=\"cpu\"}"), 40.0);
    assert!(!body.contains("wattcost_power_watts{component=\"gpu\"}"));
    assert_close(
        value(body, "wattcost_power_watts{component=\"base\"}"),
        30.0,
    );
    assert_close(
        value(body, "wattcost_power_watts{component=\"monitor\"}"),
        20.0,
    );
    assert_close(value(body, "wattcost_wall_power_watts"), 120.0);
    assert_close(
        value(body, "wattcost_last_sample_timestamp_seconds"),
        (start().timestamp() + 120) as f64,
    );
    assert_close(
        value(
            body,
            "wattcost_price_per_kwh{currency=\"USD\",period=\"flat\"}",
        ),
        0.3,
    );
}

#[test]
fn answers_only_get_and_head_on_the_metrics_path() {
    let collector = Collector::start();
    let head = collector.request("HEAD /metrics HTTP/1.1\r\n\r\n");
    assert!(
        head.starts_with("HTTP/1.1 200 OK\r\n") && head.ends_with("\r\n\r\n"),
        "{head}"
    );
    let other = collector.request("GET / HTTP/1.1\r\n\r\n");
    assert!(other.starts_with("HTTP/1.1 404 Not Found\r\n"), "{other}");
    let post = collector.request("POST /metrics HTTP/1.1\r\nContent-Length: 0\r\n\r\n");
    assert!(
        post.starts_with("HTTP/1.1 405 Method Not Allowed\r\n"),
        "{post}"
    );
}
