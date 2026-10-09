//! Cost and power over a span, on one time grid for a two-line chart.

use chrono::{DateTime, NaiveDate, TimeDelta, TimeZone, Utc};
use serde::Serialize;

use crate::config::HardwareConfig;
use crate::energy;
use crate::store::StoredSample;
use crate::time_range::{self, RangeError, Span};

const SECONDS_PER_HOUR: f64 = 3600.0;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Series {
    pub span: Span,
    pub label: String,
    pub currency: String,
    pub today_cost: f64,
    pub total_cost: f64,
    pub total_kwh: f64,
    pub average_w: Option<f64>,
    pub peak_w: Option<f64>,
    pub cpu_average_w: Option<f64>,
    pub gpu_average_w: Option<f64>,
    pub start_label: String,
    pub end_label: String,
    pub points: Vec<Point>,
}

/// One interval of the grid as rates, so a partly recorded interval is not drawn low; both are
/// `None` when nothing was recorded in it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Point {
    pub cost_per_hour: Option<f64>,
    pub average_w: Option<f64>,
    pub cpu_w: Option<f64>,
    pub gpu_w: Option<f64>,
}

/// Energy, time and cost that fell into one interval.
#[derive(Debug, Clone, Copy, Default)]
struct Totals {
    wall_wh: f64,
    duration_s: f64,
    cost: f64,
    cpu: ComponentPower,
    gpu: ComponentPower,
}

/// A component's energy and the seconds in which it was recorded.
#[derive(Debug, Clone, Copy, Default)]
struct ComponentPower {
    energy_j: f64,
    seconds: f64,
}

impl ComponentPower {
    fn add(&mut self, energy_j: Option<f64>, seconds: f64) {
        if let Some(energy_j) = energy_j {
            self.energy_j += energy_j;
            self.seconds += seconds;
        }
    }

    fn average_w(&self) -> Option<f64> {
        (self.seconds > 0.0).then(|| self.energy_j / self.seconds)
    }
}

impl Totals {
    fn add(&mut self, row: &StoredSample) {
        let sample = &row.sample;
        self.wall_wh += row.pricing.wall_wh;
        self.duration_s += sample.duration_s;
        self.cost += row.pricing.cost;
        self.cpu.add(sample.cpu.energy_j, sample.duration_s);
        self.gpu.add(sample.gpu.energy_j, sample.duration_s);
    }

    fn average_w(&self) -> Option<f64> {
        energy::average_watts(self.wall_wh, self.duration_s)
    }

    fn point(&self) -> Point {
        Point {
            cost_per_hour: (self.duration_s > 0.0)
                .then(|| self.cost * SECONDS_PER_HOUR / self.duration_s),
            average_w: self.average_w(),
            cpu_w: self.cpu.average_w(),
            gpu_w: self.gpu.average_w(),
        }
    }
}

/// Equal intervals from `from` to `to`: `bounds[i]..bounds[i + 1]` is interval `i`.
struct Grid {
    bounds: Vec<DateTime<Utc>>,
}

impl Grid {
    fn new(from: DateTime<Utc>, to: DateTime<Utc>, step: TimeDelta) -> Self {
        let mut bounds = vec![from];
        while let Some(&start) = bounds.last().filter(|start| **start < to) {
            bounds.push((start + step).min(to));
        }
        Self { bounds }
    }

    fn index(&self, time: DateTime<Utc>) -> Option<usize> {
        let after = self.bounds.partition_point(|bound| *bound <= time);
        (after > 0 && after < self.bounds.len()).then(|| after - 1)
    }

    fn totals(&self, rows: &[StoredSample]) -> Vec<Totals> {
        let mut totals = vec![Totals::default(); self.bounds.len() - 1];
        for row in rows {
            if let Some(index) = self.index(row.sample.start) {
                totals[index].add(row);
            }
        }
        totals
    }
}

/// The highest wall power of a row, from its CPU and GPU peak.
fn peak_wall_w(hardware: &HardwareConfig, row: &StoredSample) -> Option<f64> {
    let monitor_on = energy::monitor_on_s(hardware, &row.sample) > 0.0;
    row.sample
        .peak_power_w
        .map(|watts| energy::wall_watts(hardware, watts, monitor_on))
}

/// Interval length and the labels at both ends of the time axis.
fn axis(span: Span, first: NaiveDate, today: NaiveDate) -> (TimeDelta, String, String) {
    match span {
        Span::Day => (
            TimeDelta::minutes(5),
            "00:00".to_owned(),
            "24:00".to_owned(),
        ),
        Span::Week => (
            TimeDelta::hours(1),
            first.format("%a").to_string(),
            today.format("%a").to_string(),
        ),
        Span::Month => (
            TimeDelta::hours(1),
            first.format("%b %-d").to_string(),
            today.format("%b %-d").to_string(),
        ),
    }
}

/// Builds the series of `span` ending `today` from the rows of that span.
///
/// Rows in an older currency (the tariff changed within the span) are left out, so sums never
/// mix currencies.
pub fn build<Tz: TimeZone>(
    span: Span,
    today: NaiveDate,
    tz: &Tz,
    rows: &[StoredSample],
    fallback_currency: &str,
    hardware: &HardwareConfig,
) -> Result<Series, RangeError> {
    let currency = rows
        .last()
        .map_or(fallback_currency, |row| row.pricing.currency.as_str());
    let rows: Vec<StoredSample> = rows
        .iter()
        .filter(|row| row.pricing.currency == currency)
        .cloned()
        .collect();
    let range = time_range::span_range(span, today, tz)?;
    let today_start = time_range::day_start(today, tz)?;
    let (step, start_label, end_label) = axis(span, time_range::first_day(span, today), today);

    let mut total = Totals::default();
    for row in &rows {
        total.add(row);
    }
    Ok(Series {
        span,
        label: range.label,
        currency: currency.to_owned(),
        today_cost: rows
            .iter()
            .filter(|row| row.sample.start >= today_start)
            .map(|row| row.pricing.cost)
            .sum(),
        total_cost: total.cost,
        total_kwh: total.wall_wh / 1000.0,
        average_w: total.average_w(),
        peak_w: rows
            .iter()
            .filter_map(|row| peak_wall_w(hardware, row))
            .reduce(f64::max),
        cpu_average_w: total.cpu.average_w(),
        gpu_average_w: total.gpu.average_w(),
        start_label,
        end_label,
        points: Grid::new(range.from, range.to, step)
            .totals(&rows)
            .iter()
            .map(Totals::point)
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use chrono::FixedOffset;

    use super::*;
    use crate::config::HardwareConfig;
    use crate::pricing::Pricing;
    use crate::sample::RawSample;

    fn row(start: DateTime<Utc>, wall_wh: f64, cost: f64) -> StoredSample {
        StoredSample {
            id: 0,
            sample: RawSample {
                start,
                duration_s: 60.0,
                peak_power_w: Some(wall_wh * 100.0),
                ..RawSample::default()
            },
            pricing: Pricing {
                wall_wh,
                period: "day".to_owned(),
                price_per_kwh: 0.2,
                cost,
                currency: "EUR".to_owned(),
            },
        }
    }

    fn hardware() -> HardwareConfig {
        HardwareConfig {
            base_watts: 40.0,
            psu_efficiency: 0.9,
            monitor_watts: 40.0,
            ..HardwareConfig::default()
        }
    }

    fn tz() -> FixedOffset {
        FixedOffset::east_opt(2 * 3600).unwrap()
    }

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 3, 18).unwrap()
    }

    fn local(day: u32, hour: u32, minute: u32) -> DateTime<Utc> {
        tz().with_ymd_and_hms(2026, 3, day, hour, minute, 0)
            .unwrap()
            .with_timezone(&Utc)
    }

    fn series(span: Span, rows: &[StoredSample]) -> Series {
        build(span, today(), &tz(), rows, "USD", &hardware()).unwrap()
    }

    #[test]
    fn day_points_are_five_minutes_long() {
        let rows = [
            row(local(18, 0, 0), 2.0, 0.4),
            row(local(18, 0, 1), 2.0, 0.4),
            row(local(18, 13, 7), 1.0, 0.2),
        ];
        let day = series(Span::Day, &rows);
        assert_eq!(day.points.len(), 288);
        assert_eq!(
            day.points[0],
            Point {
                cost_per_hour: Some(24.0),
                average_w: Some(120.0),
                cpu_w: None,
                gpu_w: None
            }
        );
        assert_eq!(
            day.points[1],
            Point {
                cost_per_hour: None,
                average_w: None,
                cpu_w: None,
                gpu_w: None
            }
        );
        assert_eq!(day.points[13 * 12 + 1].cost_per_hour, Some(12.0));
        assert_eq!(
            (day.start_label.as_str(), day.end_label.as_str()),
            ("00:00", "24:00")
        );
        assert!((day.total_cost - 1.0).abs() < 1e-12);
        assert!((day.total_kwh - 0.005).abs() < 1e-12);
        assert_eq!(day.average_w, Some(100.0));
        let expected_peak = (200.0 + 40.0) / 0.9 + 40.0;
        assert!((day.peak_w.unwrap() - expected_peak).abs() < 1e-9);
        assert_eq!(day.currency, "EUR");
    }

    #[test]
    fn week_and_month_points_are_hourly() {
        let rows = [
            row(local(12, 10, 0), 1.0, 0.3),
            row(local(18, 23, 59), 1.0, 0.1),
        ];
        let week = series(Span::Week, &rows);
        assert_eq!(week.points.len(), 7 * 24);
        assert_eq!(week.points[10].cost_per_hour, Some(18.0));
        assert_eq!(
            (week.start_label.as_str(), week.end_label.as_str()),
            ("Thu", "Wed")
        );
        let month = series(Span::Month, &rows);
        assert_eq!(month.points.len(), 18 * 24);
        assert_eq!(
            (month.start_label.as_str(), month.end_label.as_str()),
            ("Mar 1", "Mar 18")
        );
        assert_eq!(month.points.last().unwrap().cost_per_hour, Some(6.0));
    }

    #[test]
    fn empty_range_uses_the_fallback_currency() {
        let day = series(Span::Day, &[]);
        assert_eq!(day.currency, "USD");
        assert_eq!(day.average_w, None);
        assert_eq!(day.peak_w, None);
        assert!(day.points.iter().all(|point| point.cost_per_hour.is_none()));
    }

    #[test]
    fn daylight_saving_days_have_23_and_25_hours() {
        let berlin = chrono_tz::Europe::Berlin;
        for (date, hours) in [((2026, 3, 29), 23), ((2026, 10, 25), 25)] {
            let day = NaiveDate::from_ymd_opt(date.0, date.1, date.2).unwrap();
            let series = build(Span::Day, day, &berlin, &[], "EUR", &hardware()).unwrap();
            assert_eq!(series.points.len(), hours * 12, "{day}");
        }
    }

    #[test]
    fn today_cost_is_the_part_of_the_span_from_local_midnight() {
        let rows = [
            row(local(17, 23, 59), 1.0, 0.5),
            row(local(18, 0, 0), 1.0, 0.25),
        ];
        let week = series(Span::Week, &rows);
        assert_eq!(week.today_cost, 0.25);
        assert_eq!(week.total_cost, 0.75);
    }

    #[test]
    fn only_the_latest_currency_is_summed() {
        let mut old = row(local(12, 10, 0), 1.0, 9.0);
        old.pricing.currency = "USD".to_owned();
        let week = series(Span::Week, &[old, row(local(18, 10, 0), 1.0, 0.2)]);
        assert_eq!(week.currency, "EUR");
        assert_eq!(week.total_cost, 0.2);
        assert_eq!(week.points[10].cost_per_hour, None);
    }

    #[test]
    fn a_partly_recorded_interval_keeps_its_rate() {
        let mut half = row(local(18, 0, 0), 1.0, 0.1);
        half.sample.duration_s = 30.0;
        let day = series(Span::Day, &[half]);
        assert_eq!(day.points[0].cost_per_hour, Some(12.0));
    }

    #[test]
    fn rows_on_interval_edges_land_in_the_later_interval() {
        let day = series(
            Span::Day,
            &[
                row(local(18, 0, 5), 1.0, 0.1),
                row(local(18, 23, 59), 1.0, 0.2),
            ],
        );
        assert_eq!(day.points[0].cost_per_hour, None);
        assert_eq!(day.points[1].cost_per_hour, Some(6.0));
        assert_eq!(day.points.last().unwrap().cost_per_hour, Some(12.0));
    }

    #[test]
    fn serializes_with_lowercase_span_and_null_gaps() {
        let json = serde_json::to_value(series(Span::Week, &[])).unwrap();
        assert_eq!(json["span"], "week");
        assert_eq!(json["points"][0]["cost_per_hour"], serde_json::Value::Null);
    }

    #[test]
    fn cpu_and_gpu_power_are_averaged_over_their_recorded_seconds() {
        let mut both = row(local(18, 0, 0), 1.0, 0.1);
        both.sample.cpu.energy_j = Some(3000.0);
        both.sample.gpu.energy_j = Some(1500.0);
        let mut gpu_only = row(local(18, 0, 1), 1.0, 0.1);
        gpu_only.sample.gpu.energy_j = Some(4500.0);
        let day = series(Span::Day, &[both, gpu_only]);
        assert_eq!(day.points[0].cpu_w, Some(50.0));
        assert_eq!(day.points[0].gpu_w, Some(50.0));
        assert_eq!(day.points[1].cpu_w, None);
        assert_eq!(day.cpu_average_w, Some(50.0));
        assert_eq!(day.gpu_average_w, Some(50.0));
    }

    /// The panel reads these names; renaming one must fail here, not silently in the panel.
    #[test]
    fn json_field_names_match_the_panel() {
        let keys = |value: &serde_json::Value| {
            let mut keys: Vec<String> = value.as_object().unwrap().keys().cloned().collect();
            keys.sort();
            keys
        };
        let json = serde_json::to_value(series(Span::Day, &[])).unwrap();
        assert_eq!(
            keys(&json),
            [
                "average_w",
                "cpu_average_w",
                "currency",
                "end_label",
                "gpu_average_w",
                "label",
                "peak_w",
                "points",
                "span",
                "start_label",
                "today_cost",
                "total_cost",
                "total_kwh",
            ]
        );
        assert_eq!(
            keys(&json["points"][0]),
            ["average_w", "cost_per_hour", "cpu_w", "gpu_w"]
        );
    }
}
