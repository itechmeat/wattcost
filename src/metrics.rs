//! Prometheus metrics: lifetime counters from the database and gauges from the last stored row.

use std::fmt::Write;
use std::time::Duration;

use chrono::{DateTime, TimeDelta, Utc};

use crate::config::{HardwareConfig, Settings};
use crate::energy;
use crate::pricing::Pricing;
use crate::sample::RawSample;
use crate::store::{Store, StoreError, StoredTotals};
use crate::tariff::{ClockTime, Tariff};

pub const CONTENT_TYPE: &str = "text/plain; version=0.0.4; charset=utf-8";

const JOULES_PER_WH: f64 = 3600.0;

/// Average power of one row by part.
#[derive(Debug, Clone, PartialEq)]
struct LastRow {
    end: DateTime<Utc>,
    cpu_w: Option<f64>,
    gpu_w: Option<f64>,
    base_w: f64,
    monitor_w: Option<f64>,
    wall_w: Option<f64>,
}

impl LastRow {
    fn new(sample: &RawSample, pricing: &Pricing, hardware: &HardwareConfig) -> Self {
        let seconds = sample.duration_s;
        let per_second = |joules: f64| (seconds > 0.0).then(|| joules / seconds);
        Self {
            end: sample.start
                + Duration::try_from_secs_f64(seconds)
                    .ok()
                    .and_then(|duration| TimeDelta::from_std(duration).ok())
                    .unwrap_or_default(),
            cpu_w: sample.cpu.energy_j.and_then(per_second),
            gpu_w: sample.gpu.energy_j.and_then(per_second),
            base_w: hardware.base_watts,
            monitor_w: per_second(hardware.monitor_watts * energy::monitor_on_s(hardware, sample)),
            wall_w: energy::average_watts(pricing.wall_wh, seconds),
        }
    }
}

/// What the endpoint serves; the collector updates it after every stored row.
#[derive(Debug, Clone)]
pub struct Metrics {
    totals: StoredTotals,
    last: Option<LastRow>,
    tariff: Tariff,
}

impl Metrics {
    /// Starts the counters from the stored rows, so a restart continues them instead of resetting.
    pub fn load(store: &Store, settings: &Settings) -> Result<Self, StoreError> {
        let last = store
            .last_sample()?
            .map(|row| LastRow::new(&row.sample, &row.pricing, &settings.config.hardware));
        Ok(Self {
            totals: store.totals()?,
            last,
            tariff: settings.tariff.clone(),
        })
    }

    pub fn record(&mut self, sample: &RawSample, pricing: &Pricing, hardware: &HardwareConfig) {
        let totals = &mut self.totals;
        totals.cpu_energy_j += sample.cpu.energy_j.unwrap_or(0.0);
        totals.gpu_energy_j += sample.gpu.energy_j.unwrap_or(0.0);
        totals.wall_wh += pricing.wall_wh;
        *totals.cost.entry(pricing.currency.clone()).or_default() += pricing.cost;
        self.last = Some(LastRow::new(sample, pricing, hardware));
    }

    pub fn set_tariff(&mut self, tariff: Tariff) {
        self.tariff = tariff;
    }

    /// The Prometheus text exposition format, version 0.0.4, with the price in force at `now`.
    pub fn render(&self, now: ClockTime) -> String {
        let mut out = Exposition::default();
        out.family(
            "wattcost_build_info",
            "gauge",
            "wattcost version.",
            [(vec![("version", env!("CARGO_PKG_VERSION"))], Some(1.0))],
        );
        if let Some(last) = &self.last {
            out.family(
                "wattcost_power_watts",
                "gauge",
                "Average power over the last stored row by part: cpu and gpu from sensors or estimates and base from its constant, all before power-supply losses; monitor from its constant and on-time.",
                [
                    (vec![("component", "cpu")], last.cpu_w),
                    (vec![("component", "gpu")], last.gpu_w),
                    (vec![("component", "base")], Some(last.base_w)),
                    (vec![("component", "monitor")], last.monitor_w),
                ],
            );
            out.family(
                "wattcost_wall_power_watts",
                "gauge",
                "Average power drawn from the wall over the last stored row, power-supply losses and the monitor included.",
                [(vec![], last.wall_w)],
            );
            out.family(
                "wattcost_last_sample_timestamp_seconds",
                "gauge",
                "End of the last stored row as a Unix time.",
                [(vec![], Some(last.end.timestamp_millis() as f64 / 1000.0))],
            );
        }
        out.family(
            "wattcost_energy_joules_total",
            "counter",
            "Energy of the CPU and the GPU over all stored rows, from sensors or estimates.",
            [
                (vec![("component", "cpu")], Some(self.totals.cpu_energy_j)),
                (vec![("component", "gpu")], Some(self.totals.gpu_energy_j)),
            ],
        );
        out.family(
            "wattcost_wall_energy_joules_total",
            "counter",
            "Energy drawn from the wall over all stored rows.",
            [(vec![], Some(self.totals.wall_wh * JOULES_PER_WH))],
        );
        out.family(
            "wattcost_cost_total",
            "counter",
            "Cost of all stored rows by currency, at the prices stored with each row.",
            self.totals
                .cost
                .iter()
                .map(|(currency, cost)| (vec![("currency", currency.as_str())], Some(*cost))),
        );
        let period = self.tariff.period_at(now);
        out.family(
            "wattcost_price_per_kwh",
            "gauge",
            "Price of one kWh in the tariff period in force now, fees and taxes included.",
            [(
                vec![
                    ("currency", self.tariff.currency()),
                    ("period", period.name.as_str()),
                ],
                Some(self.tariff.price_per_kwh(period)),
            )],
        );
        out.text
    }
}

#[derive(Default)]
struct Exposition {
    text: String,
}

impl Exposition {
    /// Writes one metric family; samples without a finite value are left out.
    fn family<'a>(
        &mut self,
        name: &str,
        kind: &str,
        help: &str,
        samples: impl IntoIterator<Item = (Vec<(&'a str, &'a str)>, Option<f64>)>,
    ) {
        let present: Vec<_> = samples
            .into_iter()
            .filter_map(|(labels, value)| value.filter(|v| v.is_finite()).map(|v| (labels, v)))
            .collect();
        if present.is_empty() {
            return;
        }
        let text = &mut self.text;
        let _ = writeln!(text, "# HELP {name} {}", escape(help, false));
        let _ = writeln!(text, "# TYPE {name} {kind}");
        for (labels, value) in present {
            text.push_str(name);
            if !labels.is_empty() {
                let pairs: Vec<String> = labels
                    .iter()
                    .map(|(key, value)| format!("{key}=\"{}\"", escape(value, true)))
                    .collect();
                let _ = write!(text, "{{{}}}", pairs.join(","));
            }
            let _ = writeln!(text, " {value}");
        }
    }
}

fn escape(text: &str, label: bool) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '"' if label => out.push_str("\\\""),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;
    use crate::config::EXAMPLE_CONFIG;
    use crate::sample::ComponentStats;
    use crate::test_support::TempStore;

    fn sample(cpu_j: f64) -> RawSample {
        RawSample {
            start: Utc.with_ymd_and_hms(2026, 1, 15, 12, 0, 0).unwrap(),
            duration_s: 60.0,
            cpu: ComponentStats {
                energy_j: Some(cpu_j),
                ..ComponentStats::default()
            },
            display_on_s: Some(60.0),
            ..RawSample::default()
        }
    }

    fn pricing(currency: &str) -> Pricing {
        Pricing {
            wall_wh: 1.5,
            period: "day".to_owned(),
            price_per_kwh: 0.2,
            cost: 0.0003,
            currency: currency.to_owned(),
        }
    }

    fn line<'a>(text: &'a str, prefix: &str) -> &'a str {
        text.lines()
            .find(|line| line.starts_with(prefix))
            .unwrap_or_else(|| panic!("no {prefix} in\n{text}"))
    }

    #[test]
    fn recorded_rows_continue_the_stored_totals() {
        let temp = TempStore::new();
        let store = temp.open();
        let id = store.settings_id("a = 1", Utc::now()).unwrap();
        store
            .insert_sample(&sample(600.0), &pricing("EUR"), id)
            .unwrap();
        let settings = Settings::example();
        let mut metrics = Metrics::load(&store, &settings).unwrap();
        metrics.record(&sample(1200.0), &pricing("EUR"), &settings.config.hardware);
        let text = metrics.render(ClockTime::from_hm(12, 0).unwrap());
        assert_eq!(
            line(&text, "wattcost_energy_joules_total{component=\"cpu\"}"),
            "wattcost_energy_joules_total{component=\"cpu\"} 1800"
        );
        assert_eq!(
            line(&text, "wattcost_wall_energy_joules_total "),
            "wattcost_wall_energy_joules_total 10800"
        );
        assert_eq!(
            line(&text, "wattcost_power_watts{component=\"cpu\"}"),
            "wattcost_power_watts{component=\"cpu\"} 20"
        );
    }

    #[test]
    fn label_values_are_escaped() {
        let settings =
            Settings::parse(EXAMPLE_CONFIG.replace("currency = \"EUR\"", "currency = 'a\"b\\c'"))
                .unwrap();
        let temp = TempStore::new();
        let metrics = Metrics::load(&temp.open(), &settings).unwrap();
        let text = metrics.render(ClockTime::from_hm(12, 0).unwrap());
        assert!(
            line(&text, "wattcost_price_per_kwh{")
                .starts_with("wattcost_price_per_kwh{currency=\"a\\\"b\\\\c\",period=\"day\"} "),
            "{text}"
        );
    }
}
