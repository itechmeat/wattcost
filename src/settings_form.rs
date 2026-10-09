//! The settings form: a currency, day and night periods with whole hours and final prices per
//! kWh, and the constant power of the system and the monitor.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::{fs, io};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use toml_edit::{ArrayOfTables, DocumentMut, Item, Table, value};

use crate::config::{ConfigError, EXAMPLE_CONFIG, Settings};
use crate::tariff::{ClockTime, Tariff, TariffConfig};

const DAY: &str = "day";
const NIGHT: &str = "night";
const MAX_CURRENCY_LEN: usize = 8;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SettingsForm {
    pub currency: String,
    pub day: PeriodForm,
    pub night: PeriodForm,
    /// Motherboard, memory, storage, network and fans, in watts.
    pub base_watts: f64,
    /// The monitor while it is on, in watts.
    pub monitor_watts: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PeriodForm {
    pub start_hour: u8,
    pub end_hour: u8,
    /// Final price per kWh, with discounts, fees and taxes included.
    pub price: f64,
}

#[derive(Debug, Error)]
pub enum FormError {
    #[error("the tariff in the config file cannot be edited in the form: {0}")]
    NotSimple(String),
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error("cannot parse the config file: {0}")]
    Toml(toml_edit::TomlError),
    // The fields are not named `source`, so the cause is printed once, inside the message.
    #[error("cannot read {path}: {error}")]
    Read { path: PathBuf, error: io::Error },
    #[error("cannot write {path}: {error}")]
    Write { path: PathBuf, error: io::Error },
}

impl From<toml_edit::TomlError> for FormError {
    fn from(error: toml_edit::TomlError) -> Self {
        Self::Toml(error)
    }
}

impl SettingsForm {
    /// The form view of a tariff with exactly a `day` and a `night` period on whole hours.
    pub fn from_settings(settings: &Settings) -> Result<Self, FormError> {
        let config = &settings.config.tariff;
        Ok(Self {
            currency: config.currency.clone(),
            day: period_form(config, &settings.tariff, DAY)?,
            night: period_form(config, &settings.tariff, NIGHT)?,
            base_watts: settings.config.hardware.base_watts,
            monitor_watts: settings.config.hardware.monitor_watts,
        })
    }

    /// Writes the form into the config text, keeping comments and every other setting.
    ///
    /// Prices in the form are final, so the discount, fee and tax fields are reset to neutral.
    pub fn apply(&self, text: &str) -> Result<Settings, FormError> {
        self.check()?;
        let mut document: DocumentMut = text.parse()?;
        let hardware = table(&mut document, "hardware")?;
        hardware["base_watts"] = value(self.base_watts);
        hardware["monitor_watts"] = value(self.monitor_watts);
        let tariff = table(&mut document, "tariff")?;
        tariff["currency"] = value(self.currency.trim());
        // Removing `periods` too drops the old key's formatting before the new tables go in.
        for replaced in [
            "energy_multiplier",
            "per_kwh_fees",
            "tax_multipliers",
            "periods",
        ] {
            tariff.remove(replaced);
        }
        let mut periods = ArrayOfTables::new();
        for (name, period) in [(DAY, self.day), (NIGHT, self.night)] {
            periods.push(period_table(name, period));
        }
        tariff["periods"] = Item::ArrayOfTables(periods);
        Ok(Settings::parse(document.to_string())?)
    }

    fn check(&self) -> Result<(), FormError> {
        let currency = self.currency.trim();
        if currency.is_empty() || currency.chars().count() > MAX_CURRENCY_LEN {
            return Err(FormError::Invalid(format!(
                "the currency must be 1 to {MAX_CURRENCY_LEN} characters"
            )));
        }
        for (name, period) in [(DAY, self.day), (NIGHT, self.night)] {
            if period.start_hour > 23 || period.end_hour > 23 {
                return Err(FormError::Invalid(format!(
                    "{name} hours must be between 0 and 23"
                )));
            }
            if !(period.price.is_finite() && period.price >= 0.0) {
                return Err(FormError::Invalid(format!(
                    "the {name} price must be a non-negative number"
                )));
            }
        }
        for (name, watts) in [("system", self.base_watts), ("monitor", self.monitor_watts)] {
            if !(watts.is_finite() && watts >= 0.0) {
                return Err(FormError::Invalid(format!(
                    "the {name} power must be a non-negative number of watts"
                )));
            }
        }
        Ok(())
    }
}

/// The top-level table `name`, created when missing and expanded when written inline.
fn table<'a>(document: &'a mut DocumentMut, name: &str) -> Result<&'a mut Table, FormError> {
    let item = document.entry(name).or_insert(Item::Table(Table::new()));
    if let Some(inline) = item.as_inline_table() {
        *item = Item::Table(inline.clone().into_table());
    }
    item.as_table_mut()
        .ok_or_else(|| FormError::NotSimple(format!("`{name}` is not a table")))
}

fn period_form(
    config: &TariffConfig,
    tariff: &Tariff,
    name: &str,
) -> Result<PeriodForm, FormError> {
    if config.periods.len() != 2 {
        return Err(FormError::NotSimple(format!(
            "it has {} periods instead of day and night",
            config.periods.len()
        )));
    }
    let period = config
        .periods
        .iter()
        .find(|period| period.name == name)
        .ok_or_else(|| FormError::NotSimple(format!("there is no period named {name:?}")))?;
    let hour = |time: ClockTime| {
        let minutes = time.minute_of_day();
        u8::try_from(minutes / 60)
            .ok()
            .filter(|_| minutes.is_multiple_of(60))
            .ok_or_else(|| {
                FormError::NotSimple(format!(
                    "the {name} period does not start and end on whole hours"
                ))
            })
    };
    let price = tariff.price_per_kwh(tariff.period_at(period.start));
    Ok(PeriodForm {
        start_hour: hour(period.start)?,
        end_hour: hour(period.end)?,
        price,
    })
}

fn period_table(name: &str, period: PeriodForm) -> Table {
    let mut table = Table::new();
    table["name"] = value(name);
    table["start"] = value(format!("{:02}:00", period.start_hour));
    table["end"] = value(format!("{:02}:00", period.end_hour));
    table["price"] = value(period.price);
    table
}

/// The config text to edit: the file, or the bundled example when there is none yet.
pub fn current_text(path: &Path) -> Result<String, FormError> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(EXAMPLE_CONFIG.to_owned()),
        Err(error) => Err(FormError::Read {
            path: path.to_path_buf(),
            error,
        }),
    }
}

/// Replaces the file in one step, so the running collector never reads a half-written config.
///
/// A symlinked config is written through the link, and an existing file keeps its permissions.
pub fn write_atomically(path: &Path, text: &str) -> Result<(), FormError> {
    let target = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let write_error = |error| FormError::Write {
        path: target.clone(),
        error,
    };
    let dir = target.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(dir).map_err(write_error)?;
    let mut temporary = tempfile::NamedTempFile::new_in(dir).map_err(write_error)?;
    temporary.write_all(text.as_bytes()).map_err(write_error)?;
    if let Ok(metadata) = fs::metadata(&target) {
        temporary
            .as_file()
            .set_permissions(metadata.permissions())
            .map_err(write_error)?;
    }
    temporary.as_file().sync_all().map_err(write_error)?;
    temporary
        .persist(&target)
        .map_err(|error| write_error(error.error))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn form(day: (u8, u8, f64), night: (u8, u8, f64)) -> SettingsForm {
        SettingsForm {
            currency: "EUR".to_owned(),
            day: PeriodForm {
                start_hour: day.0,
                end_hour: day.1,
                price: day.2,
            },
            night: PeriodForm {
                start_hour: night.0,
                end_hour: night.1,
                price: night.2,
            },
            base_watts: 40.0,
            monitor_watts: 40.0,
        }
    }

    #[test]
    fn shows_final_prices_of_the_example() {
        let shown = SettingsForm::from_settings(&Settings::example()).unwrap();
        assert_eq!(shown.currency, "EUR");
        assert_eq!((shown.day.start_hour, shown.day.end_hour), (7, 23));
        assert!((shown.day.price - (0.20 + 0.01) * 1.20).abs() < 1e-12);
        assert!((shown.night.price - (0.10 + 0.01) * 1.20).abs() < 1e-12);
    }

    #[test]
    fn applying_keeps_comments_and_neutralizes_price_parts() {
        let settings = form((6, 22, 0.31), (22, 6, 0.12))
            .apply(EXAMPLE_CONFIG)
            .unwrap();
        assert!(
            settings
                .text
                .contains("# Motherboard, memory, storage, network and fans, in watts.")
        );
        assert!(settings.text.contains("base_watts = 40.0"));
        assert!(!settings.text.contains("tax_multipliers"));
        let shown = SettingsForm::from_settings(&settings).unwrap();
        assert_eq!(shown, form((6, 22, 0.31), (22, 6, 0.12)));
    }

    #[test]
    fn overlapping_or_uncovered_hours_are_rejected() {
        let overlap = form((7, 23, 1.0), (22, 7, 0.5))
            .apply(EXAMPLE_CONFIG)
            .unwrap_err();
        assert!(
            overlap.to_string().contains("overlap at 22:00"),
            "{overlap}"
        );
        let gap = form((7, 22, 1.0), (23, 7, 0.5))
            .apply(EXAMPLE_CONFIG)
            .unwrap_err();
        assert!(gap.to_string().contains("no period covers 22:00"), "{gap}");
    }

    #[test]
    fn out_of_range_values_are_rejected() {
        assert!(
            form((7, 24, 1.0), (24, 7, 0.5))
                .apply(EXAMPLE_CONFIG)
                .unwrap_err()
                .to_string()
                .contains("between 0 and 23")
        );
        assert!(
            form((7, 23, -1.0), (23, 7, 0.5))
                .apply(EXAMPLE_CONFIG)
                .unwrap_err()
                .to_string()
                .contains("non-negative")
        );
        let mut long = form((7, 23, 1.0), (23, 7, 0.5));
        long.currency = "  ".to_owned();
        assert!(
            long.apply(EXAMPLE_CONFIG)
                .unwrap_err()
                .to_string()
                .contains("currency")
        );
    }

    #[test]
    fn complex_tariffs_are_left_to_the_file() {
        let three = EXAMPLE_CONFIG.replace(
            "end = \"23:00\"\nprice = 0.20",
            "end = \"18:00\"\nprice = 0.20\n\n[[tariff.periods]]\nname = \"peak\"\nstart = \"18:00\"\nend = \"23:00\"\nprice = 0.40",
        );
        let error = SettingsForm::from_settings(&Settings::parse(three).unwrap()).unwrap_err();
        assert!(error.to_string().contains("3 periods"), "{error}");
        let minutes = EXAMPLE_CONFIG.replace("\"07:00\"", "\"07:30\"");
        let error = SettingsForm::from_settings(&Settings::parse(minutes).unwrap()).unwrap_err();
        assert!(error.to_string().contains("whole hours"), "{error}");
    }

    #[test]
    fn writes_atomically_and_starts_from_the_example() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wattcost/config.toml");
        assert_eq!(current_text(&path).unwrap(), EXAMPLE_CONFIG);
        write_atomically(&path, "a = 1\n").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "a = 1\n");
    }

    #[test]
    fn writing_keeps_a_symlink_and_the_file_mode() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real.toml");
        let link = dir.path().join("link.toml");
        fs::write(&real, "a = 1\n").unwrap();
        fs::set_permissions(&real, fs::Permissions::from_mode(0o600)).unwrap();
        symlink(&real, &link).unwrap();
        write_atomically(&link, "a = 2\n").unwrap();
        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read_to_string(&real).unwrap(), "a = 2\n");
        assert_eq!(
            fs::metadata(&real).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn an_inline_tariff_table_can_be_saved() {
        let inline = "[tariff]\ncurrency = \"EUR\"\nperiods = [{ name = \"day\", start = \"07:00\", end = \"23:00\", price = 0.2 }, { name = \"night\", start = \"23:00\", end = \"07:00\", price = 0.1 }]\n";
        let settings = form((7, 23, 0.3), (23, 7, 0.1)).apply(inline).unwrap();
        assert!(
            settings.text.contains("[[tariff.periods]]"),
            "{}",
            settings.text
        );
        let top_level = "tariff = { currency = \"EUR\", periods = [{ name = \"day\", start = \"07:00\", end = \"23:00\", price = 0.2 }, { name = \"night\", start = \"23:00\", end = \"07:00\", price = 0.1 }] }\n";
        assert!(form((7, 23, 0.3), (23, 7, 0.1)).apply(top_level).is_ok());
    }

    #[test]
    fn errors_name_their_cause_once() {
        let error = FormError::from("not = [valid".parse::<DocumentMut>().unwrap_err());
        let chain = format!("{:#}", anyhow::Error::from(error));
        assert_eq!(chain.matches("expected").count(), 1, "{chain}");
        let read = current_text(Path::new("/proc/self/mem")).unwrap_err();
        assert!(read.to_string().starts_with("cannot read"), "{read}");
    }

    #[test]
    fn applying_saves_the_power_constants() {
        let mut changed = form((7, 23, 0.3), (23, 7, 0.1));
        changed.base_watts = 55.5;
        changed.monitor_watts = 32.0;
        let settings = changed.apply(EXAMPLE_CONFIG).unwrap();
        assert!(
            settings.text.contains("base_watts = 55.5"),
            "{}",
            settings.text
        );
        assert!(settings.text.contains("psu_efficiency = 0.90"));
        assert_eq!(SettingsForm::from_settings(&settings).unwrap(), changed);
        let without_hardware = "[tariff]\ncurrency = \"EUR\"\n";
        let settings = changed.apply(without_hardware).unwrap();
        assert_eq!(settings.config.hardware.monitor_watts, 32.0);
    }

    #[test]
    fn negative_power_is_rejected() {
        let mut negative = form((7, 23, 0.3), (23, 7, 0.1));
        negative.monitor_watts = -1.0;
        let error = negative.apply(EXAMPLE_CONFIG).unwrap_err();
        assert!(error.to_string().contains("monitor power"), "{error}");
    }

    /// The settings card reads and writes these names.
    #[test]
    fn json_field_names_match_the_settings_card() {
        let json = serde_json::to_value(SettingsForm::from_settings(&Settings::example()).unwrap())
            .unwrap();
        let keys = |value: &serde_json::Value| {
            value
                .as_object()
                .unwrap()
                .keys()
                .cloned()
                .collect::<Vec<_>>()
        };
        let mut top = keys(&json);
        top.sort();
        assert_eq!(
            top,
            ["base_watts", "currency", "day", "monitor_watts", "night"]
        );
        let mut period = keys(&json["day"]);
        period.sort();
        assert_eq!(period, ["end_hour", "price", "start_hour"]);
    }
}
