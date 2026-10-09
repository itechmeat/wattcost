//! Tariff periods and the effective price per kWh.

use chrono::Timelike;
use serde::Deserialize;
use thiserror::Error;

const MINUTES_PER_DAY: usize = 24 * 60;

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TariffConfig {
    pub currency: String,
    #[serde(default = "unit_multiplier")]
    pub energy_multiplier: f64,
    #[serde(default)]
    pub per_kwh_fees: Vec<f64>,
    #[serde(default)]
    pub tax_multipliers: Vec<f64>,
    #[serde(default)]
    pub periods: Vec<PeriodConfig>,
}

fn unit_multiplier() -> f64 {
    1.0
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PeriodConfig {
    pub name: String,
    pub start: ClockTime,
    pub end: ClockTime,
    pub price: f64,
}

/// A local time of day with minute precision, written as `HH:MM`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(try_from = "String")]
pub struct ClockTime(u16);

impl ClockTime {
    pub fn from_hm(hour: u32, minute: u32) -> Option<Self> {
        if hour < 24 && minute < 60 {
            u16::try_from(hour * 60 + minute).ok().map(Self)
        } else {
            None
        }
    }

    pub fn of(time: &impl Timelike) -> Self {
        Self::from_hm(time.hour(), time.minute()).expect("chrono yields hour < 24 and minute < 60")
    }

    pub fn minute_of_day(self) -> u16 {
        self.0
    }
}

impl TryFrom<String> for ClockTime {
    type Error = String;

    fn try_from(text: String) -> Result<Self, Self::Error> {
        let invalid = || format!("expected HH:MM, got {text:?}");
        let (hour, minute) = text.split_once(':').ok_or_else(invalid)?;
        if hour.len() != 2 || minute.len() != 2 {
            return Err(invalid());
        }
        let hour = hour.parse().map_err(|_| invalid())?;
        let minute = minute.parse().map_err(|_| invalid())?;
        Self::from_hm(hour, minute).ok_or_else(invalid)
    }
}

pub(crate) fn is_non_negative(value: f64) -> bool {
    value.is_finite() && value >= 0.0
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum TariffError {
    #[error("currency must not be empty")]
    EmptyCurrency,
    #[error("at least one period is required")]
    NoPeriods,
    #[error("duplicate period name {0:?}")]
    DuplicateName(String),
    #[error("{0} must be a non-negative number")]
    Negative(String),
    #[error("periods {first:?} and {second:?} overlap at {at}")]
    Overlap {
        first: String,
        second: String,
        at: String,
    },
    #[error("no period covers {0}")]
    Gap(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Period {
    pub name: String,
    pub price: f64,
}

#[derive(Debug, Clone)]
pub struct Tariff {
    currency: String,
    energy_multiplier: f64,
    fees_per_kwh: f64,
    tax_factor: f64,
    periods: Vec<Period>,
    period_by_minute: Vec<usize>,
}

impl Tariff {
    pub fn new(config: &TariffConfig) -> Result<Self, TariffError> {
        if config.currency.trim().is_empty() {
            return Err(TariffError::EmptyCurrency);
        }
        if config.periods.is_empty() {
            return Err(TariffError::NoPeriods);
        }
        check_non_negative("tariff.energy_multiplier", config.energy_multiplier)?;
        for fee in &config.per_kwh_fees {
            check_non_negative("tariff.per_kwh_fees", *fee)?;
        }
        for tax in &config.tax_multipliers {
            check_non_negative("tariff.tax_multipliers", *tax)?;
        }

        let mut owner: Vec<Option<usize>> = vec![None; MINUTES_PER_DAY];
        for (index, period) in config.periods.iter().enumerate() {
            check_non_negative(&format!("price of period {:?}", period.name), period.price)?;
            if config.periods[..index]
                .iter()
                .any(|earlier| earlier.name == period.name)
            {
                return Err(TariffError::DuplicateName(period.name.clone()));
            }
            for minute in minutes_of(period) {
                if let Some(other) = owner[minute] {
                    return Err(TariffError::Overlap {
                        first: config.periods[other].name.clone(),
                        second: period.name.clone(),
                        at: format_minute(minute),
                    });
                }
                owner[minute] = Some(index);
            }
        }
        let period_by_minute = owner
            .into_iter()
            .enumerate()
            .map(|(minute, index)| index.ok_or_else(|| TariffError::Gap(format_minute(minute))))
            .collect::<Result<_, _>>()?;

        Ok(Self {
            currency: config.currency.clone(),
            energy_multiplier: config.energy_multiplier,
            fees_per_kwh: config.per_kwh_fees.iter().sum(),
            tax_factor: config.tax_multipliers.iter().product(),
            periods: config
                .periods
                .iter()
                .map(|period| Period {
                    name: period.name.clone(),
                    price: period.price,
                })
                .collect(),
            period_by_minute,
        })
    }

    pub fn currency(&self) -> &str {
        &self.currency
    }

    pub fn period_at(&self, time: ClockTime) -> &Period {
        &self.periods[self.period_by_minute[usize::from(time.minute_of_day())]]
    }

    pub fn price_per_kwh(&self, period: &Period) -> f64 {
        (period.price * self.energy_multiplier + self.fees_per_kwh) * self.tax_factor
    }
}

/// Minutes of the day covered by a period; `start == end` covers the whole day.
fn minutes_of(period: &PeriodConfig) -> impl Iterator<Item = usize> {
    let start = usize::from(period.start.minute_of_day());
    let end = usize::from(period.end.minute_of_day());
    let length = if end > start {
        end - start
    } else {
        MINUTES_PER_DAY - start + end
    };
    (start..start + length).map(|minute| minute % MINUTES_PER_DAY)
}

fn format_minute(minute: usize) -> String {
    format!("{:02}:{:02}", minute / 60, minute % 60)
}

fn check_non_negative(what: &str, value: f64) -> Result<(), TariffError> {
    if is_non_negative(value) {
        Ok(())
    } else {
        Err(TariffError::Negative(what.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Result<Tariff, String> {
        let config: TariffConfig = toml::from_str(text).map_err(|error| error.to_string())?;
        Tariff::new(&config).map_err(|error| error.to_string())
    }

    fn tariff(periods: &str) -> Result<Tariff, String> {
        parse(&format!("currency = \"EUR\"\n{periods}"))
    }

    fn at(hour: u32, minute: u32) -> ClockTime {
        ClockTime::from_hm(hour, minute).unwrap()
    }

    const DAY_NIGHT: &str = r#"
[[periods]]
name = "day"
start = "07:00"
end = "23:00"
price = 0.20
[[periods]]
name = "night"
start = "23:00"
end = "07:00"
price = 0.10
"#;

    #[test]
    fn boundary_minute_belongs_to_starting_period() {
        let t = tariff(DAY_NIGHT).unwrap();
        assert_eq!(t.period_at(at(22, 59)).name, "day");
        assert_eq!(t.period_at(at(23, 0)).name, "night");
        assert_eq!(t.period_at(at(7, 0)).name, "day");
        assert_eq!(t.period_at(at(6, 59)).name, "night");
    }

    #[test]
    fn period_crossing_midnight() {
        let t = tariff(DAY_NIGHT).unwrap();
        assert_eq!(t.period_at(at(0, 0)).name, "night");
        assert_eq!(t.period_at(at(23, 59)).name, "night");
    }

    #[test]
    fn equal_start_and_end_cover_the_whole_day() {
        let t = tariff(
            "[[periods]]\nname = \"flat\"\nstart = \"00:00\"\nend = \"00:00\"\nprice = 0.3\n",
        )
        .unwrap();
        assert_eq!(t.period_at(at(13, 37)).name, "flat");
    }

    #[test]
    fn price_applies_multiplier_then_fees_then_taxes() {
        let t = &parse(&format!(
            "currency = \"EUR\"\nenergy_multiplier = 0.95\nper_kwh_fees = [0.801, 0.015]\ntax_multipliers = [1.075, 1.2]\n{DAY_NIGHT}"
        ))
        .unwrap();
        let day = t.period_at(at(12, 0));
        let expected = (0.20 * 0.95 + 0.816) * 1.075 * 1.2;
        assert!((t.price_per_kwh(day) - expected).abs() < 1e-12);
    }

    #[test]
    fn gap_is_rejected() {
        let err = tariff(
            "[[periods]]\nname = \"day\"\nstart = \"07:00\"\nend = \"23:00\"\nprice = 0.2\n",
        )
        .unwrap_err();
        assert!(err.to_string().contains("no period covers 00:00"), "{err}");
    }

    #[test]
    fn overlap_is_rejected() {
        let err = tariff(&format!("{DAY_NIGHT}[[periods]]\nname = \"peak\"\nstart = \"18:00\"\nend = \"19:00\"\nprice = 0.5\n")).unwrap_err();
        assert!(err.to_string().contains("overlap at 18:00"), "{err}");
    }

    #[test]
    fn duplicate_names_and_negative_prices_are_rejected() {
        let dup = DAY_NIGHT.replace("\"night\"", "\"day\"");
        assert!(
            tariff(&dup)
                .unwrap_err()
                .to_string()
                .contains("duplicate period name")
        );
        let negative = DAY_NIGHT.replace("0.10", "-0.10");
        assert!(
            tariff(&negative)
                .unwrap_err()
                .to_string()
                .contains("non-negative")
        );
    }

    #[test]
    fn clock_time_parsing() {
        assert_eq!(
            ClockTime::try_from("07:30".to_owned())
                .unwrap()
                .minute_of_day(),
            450
        );
        for bad in ["7:30", "24:00", "12:60", "noon", "12-00"] {
            assert!(ClockTime::try_from(bad.to_owned()).is_err(), "{bad}");
        }
    }
}
