//! A one-line status of today's cost and the current power, for status bars such as Waybar.

use chrono::{DateTime, TimeDelta, Utc};
use clap::ValueEnum;
use serde::Serialize;

use crate::energy;
use crate::series::Series;
use crate::store::StoredSample;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Format {
    /// `1.23 EUR · 45 W`
    Text,
    /// Waybar custom-module JSON: `text`, `tooltip` and `class` (`ok`, or `stale` when no row was
    /// written lately)
    Waybar,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Status {
    pub currency: String,
    pub today_cost: f64,
    pub today_kwh: f64,
    /// Wall power of the last stored row; `None` when that row is older than the allowed age.
    pub now_w: Option<f64>,
    pub average_w: Option<f64>,
    pub peak_w: Option<f64>,
}

#[derive(Serialize)]
struct Waybar<'a> {
    text: &'a str,
    tooltip: String,
    class: &'static str,
}

/// Today's totals from the day series and the current power from the last row, if it ended no
/// more than `max_age` before `now`.
pub fn build(
    day: &Series,
    last: Option<&StoredSample>,
    now: DateTime<Utc>,
    max_age: TimeDelta,
) -> Status {
    let now_w = last
        .filter(|row| {
            let since_end_s = (now - row.sample.start).as_seconds_f64() - row.sample.duration_s;
            since_end_s <= max_age.as_seconds_f64()
        })
        .and_then(|row| energy::average_watts(row.pricing.wall_wh, row.sample.duration_s));
    Status {
        currency: day.currency.clone(),
        today_cost: day.today_cost,
        today_kwh: day.total_kwh,
        now_w,
        average_w: day.average_w,
        peak_w: day.peak_w,
    }
}

impl Status {
    pub fn text(&self) -> String {
        let cost = format!("{:.2} {}", self.today_cost, self.currency);
        match self.now_w {
            Some(watts) => format!("{cost} · {watts:.0} W"),
            None => cost,
        }
    }

    pub fn render(&self, format: Format) -> String {
        match format {
            Format::Text => self.text(),
            Format::Waybar => {
                let text = self.text();
                let waybar = Waybar {
                    text: &text,
                    tooltip: self.tooltip(),
                    class: if self.now_w.is_some() { "ok" } else { "stale" },
                };
                serde_json::to_string(&waybar).expect("a struct of strings serializes")
            }
        }
    }

    fn tooltip(&self) -> String {
        let mut lines = vec![format!(
            "Today: {:.2} {}, {:.3} kWh",
            self.today_cost, self.currency, self.today_kwh
        )];
        lines.push(match self.now_w {
            Some(watts) => format!("Now: {watts:.0} W"),
            None => "Now: no recent data; is the wattcost service running?".to_owned(),
        });
        if let (Some(average), Some(peak)) = (self.average_w, self.peak_w) {
            lines.push(format!("Average: {average:.0} W, peak: {peak:.0} W"));
        }
        lines.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;
    use crate::pricing::Pricing;
    use crate::sample::RawSample;
    use crate::time_range::Span;

    fn day() -> Series {
        Series {
            span: Span::Day,
            label: "2026-03-18".to_owned(),
            currency: "EUR".to_owned(),
            today_cost: 1.234,
            total_cost: 1.234,
            total_kwh: 4.5,
            average_w: Some(187.4),
            peak_w: Some(410.0),
            cpu_average_w: None,
            gpu_average_w: None,
            start_label: "00:00".to_owned(),
            end_label: "24:00".to_owned(),
            points: Vec::new(),
        }
    }

    fn row_ending_at(end: DateTime<Utc>, wall_wh: f64) -> StoredSample {
        StoredSample {
            id: 1,
            sample: RawSample {
                start: end - TimeDelta::seconds(60),
                duration_s: 60.0,
                ..RawSample::default()
            },
            pricing: Pricing {
                wall_wh,
                period: "day".to_owned(),
                price_per_kwh: 0.2,
                cost: 0.0,
                currency: "EUR".to_owned(),
            },
        }
    }

    fn noon() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 3, 18, 12, 0, 0).unwrap()
    }

    #[test]
    fn a_recent_row_gives_the_current_power() {
        let row = row_ending_at(noon() - TimeDelta::seconds(30), 1.5);
        let status = build(&day(), Some(&row), noon(), TimeDelta::minutes(3));
        assert_eq!(status.now_w, Some(90.0));
        assert_eq!(status.render(Format::Text), "1.23 EUR · 90 W");
    }

    #[test]
    fn an_old_row_or_none_shows_only_the_cost() {
        let old = row_ending_at(noon() - TimeDelta::minutes(4), 1.5);
        for last in [Some(&old), None] {
            let status = build(&day(), last, noon(), TimeDelta::minutes(3));
            assert_eq!(status.now_w, None);
            assert_eq!(status.render(Format::Text), "1.23 EUR");
        }
    }

    #[test]
    fn waybar_json_carries_text_tooltip_and_class() {
        let row = row_ending_at(noon(), 1.5);
        let fresh = build(&day(), Some(&row), noon(), TimeDelta::minutes(3));
        let json: serde_json::Value = serde_json::from_str(&fresh.render(Format::Waybar)).unwrap();
        assert_eq!(json["text"], "1.23 EUR · 90 W");
        assert_eq!(json["class"], "ok");
        assert_eq!(
            json["tooltip"],
            "Today: 1.23 EUR, 4.500 kWh\nNow: 90 W\nAverage: 187 W, peak: 410 W"
        );

        let stale = build(&day(), None, noon(), TimeDelta::minutes(3));
        let json: serde_json::Value = serde_json::from_str(&stale.render(Format::Waybar)).unwrap();
        assert_eq!(json["class"], "stale");
        assert!(json["tooltip"].as_str().unwrap().contains("no recent data"));
    }
}
