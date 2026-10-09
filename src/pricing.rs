//! Prices one row with the tariff in force at its local start time.

use chrono::TimeZone;

use crate::config::Settings;
use crate::energy;
use crate::sample::RawSample;
use crate::tariff::ClockTime;

#[derive(Debug, Clone, PartialEq)]
pub struct Pricing {
    pub wall_wh: f64,
    pub period: String,
    pub price_per_kwh: f64,
    pub cost: f64,
    pub currency: String,
}

pub fn price<Tz: TimeZone>(settings: &Settings, sample: &RawSample, tz: &Tz) -> Pricing {
    let wall_wh = energy::wall_wh(&settings.config.hardware, sample);
    let period = settings
        .tariff
        .period_at(ClockTime::of(&sample.start.with_timezone(tz)));
    let price_per_kwh = settings.tariff.price_per_kwh(period);
    Pricing {
        wall_wh,
        period: period.name.clone(),
        price_per_kwh,
        cost: wall_wh / 1000.0 * price_per_kwh,
        currency: settings.tariff.currency().to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use chrono::{FixedOffset, TimeZone, Utc};

    use super::*;
    use crate::config::Settings;
    use crate::sample::{ComponentStats, RawSample};

    #[test]
    fn uses_local_time_for_the_period() {
        let settings = Settings::example();
        let sample = RawSample {
            start: Utc.with_ymd_and_hms(2026, 1, 15, 21, 30, 0).unwrap(),
            duration_s: 60.0,
            cpu: ComponentStats {
                energy_j: Some(0.0),
                ..ComponentStats::default()
            },
            gpu: ComponentStats::default(),
            system: ComponentStats {
                energy_j: Some(3600.0 * 1000.0),
                ..ComponentStats::default()
            },
            display_on_s: Some(0.0),
            system_covered_s: 60.0,
            peak_power_w: None,
        };
        let plus_two = FixedOffset::east_opt(2 * 3600).unwrap();
        let pricing = price(&settings, &sample, &plus_two);
        assert_eq!(pricing.period, "night");
        let expected_price = (0.10 + 0.01) * 1.20;
        assert!((pricing.price_per_kwh - expected_price).abs() < 1e-12);
        assert!((pricing.wall_wh - 1000.0).abs() < 1e-9);
        assert!((pricing.cost - expected_price).abs() < 1e-12);
        assert_eq!(pricing.currency, "EUR");
        assert_eq!(price(&settings, &sample, &Utc).period, "day");
    }
}
