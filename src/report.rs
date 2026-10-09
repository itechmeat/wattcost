//! Energy and cost per tariff period over a range of stored rows.

use std::collections::BTreeMap;
use std::fmt;

use crate::config::HardwareConfig;
use crate::energy;
use crate::providers::Quality;
use crate::sample::ComponentStats;
use crate::store::StoredSample;

const SECONDS_PER_HOUR: f64 = 3600.0;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Line {
    pub label: String,
    pub currency: String,
    pub duration_s: f64,
    pub monitor_on_s: f64,
    pub wall_wh: f64,
    pub cost: f64,
    /// Highest combined CPU and GPU power seen in one sampling interval.
    pub peak_w: Option<f64>,
}

impl Line {
    fn new(label: &str, currency: &str) -> Self {
        Self {
            label: label.to_owned(),
            currency: currency.to_owned(),
            ..Self::default()
        }
    }

    fn add(&mut self, row: &StoredSample, hardware: &HardwareConfig) {
        let sample = &row.sample;
        self.duration_s += sample.duration_s;
        self.monitor_on_s += energy::monitor_on_s(hardware, sample);
        self.wall_wh += row.pricing.wall_wh;
        self.cost += row.pricing.cost;
        self.peak_w = match (self.peak_w, sample.peak_power_w) {
            (Some(current), Some(new)) => Some(current.max(new)),
            (current, new) => current.or(new),
        };
    }

    pub fn average_w(&self) -> f64 {
        energy::average_watts(self.wall_wh, self.duration_s).unwrap_or(0.0)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    pub label: String,
    pub lines: Vec<Line>,
    pub totals: Vec<Line>,
    /// Seconds per data quality, for the CPU and the GPU.
    pub quality: [(&'static str, BTreeMap<Quality, f64>); 2],
}

pub fn summarize(label: String, rows: &[StoredSample], hardware: &HardwareConfig) -> Report {
    let mut lines: BTreeMap<(String, String), Line> = BTreeMap::new();
    let mut totals: BTreeMap<String, Line> = BTreeMap::new();
    let mut quality = [("CPU", BTreeMap::new()), ("GPU", BTreeMap::new())];
    for row in rows {
        let (period, currency) = (&row.pricing.period, &row.pricing.currency);
        lines
            .entry((period.clone(), currency.clone()))
            .or_insert_with(|| Line::new(period, currency))
            .add(row, hardware);
        totals
            .entry(currency.clone())
            .or_insert_with(|| Line::new("total", currency))
            .add(row, hardware);
        for ((_, seconds), stats) in quality.iter_mut().zip([&row.sample.cpu, &row.sample.gpu]) {
            add_quality(seconds, stats, row.sample.duration_s);
        }
    }
    Report {
        label,
        lines: lines.into_values().collect(),
        totals: totals.into_values().collect(),
        quality,
    }
}

fn add_quality(seconds: &mut BTreeMap<Quality, f64>, stats: &ComponentStats, duration_s: f64) {
    *seconds.entry(stats.quality).or_insert(0.0) += duration_s;
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "{} (local time)", self.label)?;
        if self.lines.is_empty() {
            return writeln!(f, "No data in this range.");
        }
        writeln!(
            f,
            "{:<12} {:>7} {:>10} {:>7} {:>15} {:>9} {:>14}",
            "Period", "Hours", "Monitor h", "Avg W", "Peak CPU+GPU W", "kWh", "Cost"
        )?;
        for line in self.lines.iter().chain(&self.totals) {
            let peak = line
                .peak_w
                .map_or_else(|| "-".to_owned(), |watts| format!("{watts:.0}"));
            writeln!(
                f,
                "{:<12} {:>7.2} {:>10.2} {:>7.0} {:>15} {:>9.3} {:>10.2} {}",
                line.label,
                line.duration_s / SECONDS_PER_HOUR,
                line.monitor_on_s / SECONDS_PER_HOUR,
                line.average_w(),
                peak,
                line.wall_wh / 1000.0,
                line.cost,
                line.currency,
            )?;
        }
        for (component, seconds) in &self.quality {
            let total: f64 = seconds.values().sum();
            let shares: Vec<String> = seconds
                .iter()
                .rev()
                .filter(|(_, value)| **value > 0.0)
                .map(|(quality, value)| {
                    format!("{} {:.1}%", quality.label(), 100.0 * value / total)
                })
                .collect();
            writeln!(f, "{component}: {}", shares.join(", "))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use super::*;
    use crate::config::HardwareConfig;
    use crate::pricing::Pricing;
    use crate::providers::Quality;
    use crate::sample::{ComponentStats, RawSample};

    fn assumed(on: bool) -> HardwareConfig {
        HardwareConfig {
            monitor_assumed_on: on,
            ..HardwareConfig::default()
        }
    }

    fn row(
        id: i64,
        period: &str,
        wall_wh: f64,
        cost: f64,
        display_on_s: Option<f64>,
    ) -> StoredSample {
        StoredSample {
            id,
            sample: RawSample {
                start: Utc.with_ymd_and_hms(2026, 1, 15, 12, 0, 0).unwrap(),
                duration_s: 60.0,
                cpu: ComponentStats {
                    power_max_w: Some(80.0),
                    quality: Quality::Measured,
                    ..ComponentStats::default()
                },
                gpu: ComponentStats {
                    power_max_w: Some(250.0),
                    quality: Quality::Measured,
                    ..ComponentStats::default()
                },
                system: ComponentStats::default(),
                display_on_s,
                system_covered_s: 0.0,
                peak_power_w: Some(310.0),
            },
            pricing: Pricing {
                wall_wh,
                period: period.to_owned(),
                price_per_kwh: 0.2,
                cost,
                currency: "EUR".to_owned(),
            },
        }
    }

    #[test]
    fn groups_by_period_and_totals() {
        let report = summarize(
            "2026-01-15".to_owned(),
            &[
                row(1, "day", 2.0, 0.4, Some(60.0)),
                row(2, "night", 1.0, 0.1, Some(0.0)),
                row(3, "day", 2.0, 0.4, None),
            ],
            &assumed(true),
        );
        let day = report
            .lines
            .iter()
            .find(|line| line.label == "day")
            .unwrap();
        assert_eq!(day.duration_s, 120.0);
        assert_eq!(day.wall_wh, 4.0);
        assert!((day.cost - 0.8).abs() < 1e-12);
        assert_eq!(day.monitor_on_s, 120.0);
        assert_eq!(day.peak_w, Some(310.0));
        assert!((day.average_w() - 120.0).abs() < 1e-9);
        assert_eq!(report.totals.len(), 1);
        assert!((report.totals[0].cost - 0.9).abs() < 1e-12);
    }

    #[test]
    fn report_counts_duplicate_minute_rows() {
        let report = summarize(
            String::new(),
            &[row(1, "day", 1.0, 0.2, None), row(2, "day", 1.0, 0.2, None)],
            &assumed(true),
        );
        assert_eq!(report.totals[0].wall_wh, 2.0);
    }

    #[test]
    fn empty_range_renders_a_message() {
        let text = summarize("2026-01-15".to_owned(), &[], &assumed(true)).to_string();
        assert!(text.contains("No data"), "{text}");
    }

    #[test]
    fn renders_quality_shares() {
        let text = summarize(
            "x".to_owned(),
            &[row(1, "day", 1.0, 0.2, None)],
            &assumed(true),
        )
        .to_string();
        assert!(text.contains("CPU: measured 100.0%"), "{text}");
    }

    #[test]
    fn unknown_display_follows_the_monitor_assumption() {
        let rows = [row(1, "day", 1.0, 0.2, None)];
        assert_eq!(
            summarize(String::new(), &rows, &assumed(false)).totals[0].monitor_on_s,
            0.0
        );
        assert_eq!(
            summarize(String::new(), &rows, &assumed(true)).totals[0].monitor_on_s,
            60.0
        );
    }
}
