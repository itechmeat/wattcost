//! Recomputes stored prices for a range with the current settings.

use std::collections::BTreeMap;

use chrono::{DateTime, TimeZone, Utc};

use crate::config::Settings;
use crate::pricing::{self, Pricing};
use crate::store::{Store, StoreError};
use crate::time_range::Range;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Totals {
    pub wall_wh: f64,
    pub cost: BTreeMap<String, f64>,
}

impl Totals {
    fn of<'a>(pricings: impl Iterator<Item = &'a Pricing>) -> Self {
        pricings.fold(Self::default(), |mut totals, pricing| {
            totals.wall_wh += pricing.wall_wh;
            *totals.cost.entry(pricing.currency.clone()).or_insert(0.0) += pricing.cost;
            totals
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RepriceOutcome {
    pub rows: usize,
    pub before: Totals,
    pub after: Totals,
    pub applied: bool,
}

pub fn reprice<Tz: TimeZone>(
    store: &mut Store,
    settings: &Settings,
    tz: &Tz,
    range: &Range,
    dry_run: bool,
    now: DateTime<Utc>,
) -> Result<RepriceOutcome, StoreError> {
    let rows = store.samples(range.from, range.to)?;
    let repriced: Vec<(i64, Pricing)> = rows
        .iter()
        .map(|row| (row.id, pricing::price(settings, &row.sample, tz)))
        .collect();
    let applied = !dry_run && !repriced.is_empty();
    if applied {
        let settings_id = store.settings_id(&settings.text, now)?;
        store.update_pricing(&repriced, settings_id)?;
    }
    Ok(RepriceOutcome {
        rows: rows.len(),
        before: Totals::of(rows.iter().map(|row| &row.pricing)),
        after: Totals::of(repriced.iter().map(|(_, pricing)| pricing)),
        applied,
    })
}

#[cfg(test)]
mod tests {
    use chrono::{TimeDelta, TimeZone, Utc};

    use super::*;
    use crate::config::{EXAMPLE_CONFIG, Settings};
    use crate::sample::{ComponentStats, RawSample};
    use crate::test_support::TempStore;
    use crate::time_range::Range;

    fn setup() -> (TempStore, Store, Range) {
        let temp = TempStore::new();
        let store = temp.open();
        let start = Utc.with_ymd_and_hms(2026, 1, 15, 12, 0, 0).unwrap();
        let settings = Settings::example();
        let id = store.settings_id(&settings.text, start).unwrap();
        let sample = RawSample {
            start,
            duration_s: 60.0,
            cpu: ComponentStats::default(),
            gpu: ComponentStats::default(),
            system: ComponentStats {
                energy_j: Some(3600.0 * 1000.0),
                ..ComponentStats::default()
            },
            display_on_s: Some(0.0),
            system_covered_s: 60.0,
            peak_power_w: None,
        };
        store
            .insert_sample(&sample, &pricing::price(&settings, &sample, &Utc), id)
            .unwrap();
        (
            temp,
            store,
            Range {
                from: start,
                to: start + TimeDelta::minutes(1),
                label: String::new(),
            },
        )
    }

    fn doubled_prices() -> Settings {
        Settings::parse(EXAMPLE_CONFIG.replace("price = 0.20", "price = 0.40")).unwrap()
    }

    #[test]
    fn dry_run_reports_without_writing() {
        let (_temp, mut store, range) = setup();
        let outcome = reprice(
            &mut store,
            &doubled_prices(),
            &Utc,
            &range,
            true,
            range.from,
        )
        .unwrap();
        assert_eq!(outcome.rows, 1);
        assert!(!outcome.applied);
        assert!(outcome.after.cost["EUR"] > outcome.before.cost["EUR"]);
        let stored = store.samples(range.from, range.to).unwrap();
        assert!((stored[0].pricing.cost - outcome.before.cost["EUR"]).abs() < 1e-12);
    }

    #[test]
    fn applies_new_prices_and_keeps_raw_columns() {
        let (_temp, mut store, range) = setup();
        let before = store.samples(range.from, range.to).unwrap();
        let outcome = reprice(
            &mut store,
            &doubled_prices(),
            &Utc,
            &range,
            false,
            range.from,
        )
        .unwrap();
        assert!(outcome.applied);
        let after = store.samples(range.from, range.to).unwrap();
        assert_eq!(after[0].sample, before[0].sample);
        assert!((after[0].pricing.cost - (0.40 + 0.01) * 1.2).abs() < 1e-9);
    }

    #[test]
    fn an_empty_range_records_no_new_settings() {
        let (_temp, mut store, range) = setup();
        let original = store
            .settings_id(&Settings::example().text, range.from)
            .unwrap();
        let later = Range {
            from: range.to,
            to: range.to + TimeDelta::hours(1),
            label: String::new(),
        };
        let outcome = reprice(
            &mut store,
            &doubled_prices(),
            &Utc,
            &later,
            false,
            range.from,
        )
        .unwrap();
        assert!(!outcome.applied);
        assert_eq!(
            store
                .settings_id(&Settings::example().text, range.from)
                .unwrap(),
            original
        );
    }
}
