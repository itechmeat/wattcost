//! User configuration: collector timing, hardware constants and the electricity tariff.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::{fs, io};

use serde::Deserialize;
use thiserror::Error;

use crate::tariff::{Tariff, TariffConfig, TariffError, is_non_negative};

/// Neutral example configuration, also used when no config file exists.
pub const EXAMPLE_CONFIG: &str = include_str!("../config.example.toml");

/// Names accepted in `hardware.providers`.
pub const PROVIDER_NAMES: [&str; 7] = [
    "rapl",
    "cpu_stat",
    "cpu_estimate",
    "nvml",
    "amdgpu",
    "battery",
    "display",
];

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("cannot read {path}: {error}")]
    Read { path: PathBuf, error: io::Error },
    #[error("invalid TOML: {0}")]
    Parse(toml::de::Error),
    #[error("invalid config: {0}")]
    Invalid(String),
    #[error("invalid tariff: {0}")]
    Tariff(TariffError),
}

// Written by hand instead of `#[from]`: the message already includes the inner error, and
// `#[from]` would also report it as the source, so it would be printed twice.
impl From<toml::de::Error> for ConfigError {
    fn from(error: toml::de::Error) -> Self {
        Self::Parse(error)
    }
}

impl From<TariffError> for ConfigError {
    fn from(error: TariffError) -> Self {
        Self::Tariff(error)
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub collector: CollectorConfig,
    #[serde(default)]
    pub hardware: HardwareConfig,
    pub tariff: TariffConfig,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CollectorConfig {
    pub sample_interval_s: u32,
    pub write_interval_s: u32,
    pub database: Option<PathBuf>,
}

impl Default for CollectorConfig {
    fn default() -> Self {
        Self {
            sample_interval_s: 2,
            write_interval_s: 60,
            database: None,
        }
    }
}

impl CollectorConfig {
    pub fn database_path(&self) -> PathBuf {
        self.database
            .as_deref()
            .map_or_else(|| data_dir().join("wattcost.db"), expand_home)
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HardwareConfig {
    pub base_watts: f64,
    pub psu_efficiency: f64,
    pub monitor_watts: f64,
    pub monitor_assumed_on: bool,
    pub cpu_idle_watts: f64,
    pub cpu_max_watts: f64,
    pub providers: BTreeMap<String, bool>,
}

impl Default for HardwareConfig {
    fn default() -> Self {
        Self {
            base_watts: 40.0,
            psu_efficiency: 0.90,
            monitor_watts: 40.0,
            monitor_assumed_on: true,
            cpu_idle_watts: 20.0,
            cpu_max_watts: 120.0,
            providers: BTreeMap::new(),
        }
    }
}

impl HardwareConfig {
    pub fn provider_enabled(&self, name: &str) -> bool {
        self.providers.get(name).copied().unwrap_or(true)
    }

    /// True only when the config switches the provider on explicitly.
    pub fn provider_forced(&self, name: &str) -> bool {
        self.providers.get(name).copied().unwrap_or(false)
    }
}

/// A validated configuration, the tariff built from it and the text it came from.
#[derive(Debug, Clone)]
pub struct Settings {
    pub config: Config,
    pub tariff: Tariff,
    pub text: String,
}

impl Settings {
    pub fn parse(text: impl Into<String>) -> Result<Self, ConfigError> {
        let text = text.into();
        let config: Config = toml::from_str(&text)?;
        validate(&config)?;
        let tariff = Tariff::new(&config.tariff)?;
        Ok(Self {
            config,
            tariff,
            text,
        })
    }

    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let text = fs::read_to_string(path).map_err(|error| ConfigError::Read {
            path: path.to_path_buf(),
            error,
        })?;
        Self::parse(text)
    }

    pub fn example() -> Self {
        Self::parse(EXAMPLE_CONFIG).expect("the bundled example config is valid")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigSource {
    File(PathBuf),
    Example { missing: PathBuf },
}

pub fn load_or_example(path: &Path) -> Result<(Settings, ConfigSource), ConfigError> {
    match Settings::load(path) {
        Ok(settings) => Ok((settings, ConfigSource::File(path.to_path_buf()))),
        Err(ConfigError::Read { error, .. }) if error.kind() == io::ErrorKind::NotFound => Ok((
            Settings::example(),
            ConfigSource::Example {
                missing: path.to_path_buf(),
            },
        )),
        Err(error) => Err(error),
    }
}

pub fn default_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| home_dir().join(".config"))
        .join("wattcost/config.toml")
}

fn data_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| home_dir().join(".local/share"))
        .join("wattcost")
}

fn home_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

fn expand_home(path: &Path) -> PathBuf {
    path.strip_prefix("~")
        .map_or_else(|_| path.to_path_buf(), |rest| home_dir().join(rest))
}

fn validate(config: &Config) -> Result<(), ConfigError> {
    let collector = &config.collector;
    ensure(
        collector.sample_interval_s > 0,
        "collector.sample_interval_s must be positive",
    )?;
    ensure(
        collector.write_interval_s > 0 && 60_u32.is_multiple_of(collector.write_interval_s),
        "collector.write_interval_s must divide 60 so that a row never spans two tariff periods",
    )?;
    ensure(
        collector
            .write_interval_s
            .is_multiple_of(collector.sample_interval_s),
        "collector.write_interval_s must be a multiple of sample_interval_s",
    )?;

    let hardware = &config.hardware;
    ensure(
        hardware.psu_efficiency > 0.0 && hardware.psu_efficiency <= 1.0,
        "hardware.psu_efficiency must be in (0, 1]",
    )?;
    for (name, value) in [
        ("base_watts", hardware.base_watts),
        ("monitor_watts", hardware.monitor_watts),
        ("cpu_idle_watts", hardware.cpu_idle_watts),
        ("cpu_max_watts", hardware.cpu_max_watts),
    ] {
        ensure(
            is_non_negative(value),
            &format!("hardware.{name} must be a non-negative number"),
        )?;
    }
    ensure(
        hardware.cpu_max_watts >= hardware.cpu_idle_watts,
        "hardware.cpu_max_watts must not be below cpu_idle_watts",
    )?;
    for name in hardware.providers.keys() {
        ensure(
            PROVIDER_NAMES.contains(&name.as_str()),
            &format!("hardware.providers: unknown provider {name:?}"),
        )?;
    }
    Ok(())
}

fn ensure(condition: bool, message: &str) -> Result<(), ConfigError> {
    if condition {
        Ok(())
    } else {
        Err(ConfigError::Invalid(message.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn example_config_is_valid() {
        assert!(Settings::parse(EXAMPLE_CONFIG).is_ok());
    }

    #[test]
    fn write_interval_must_divide_60_and_be_a_multiple_of_sample() {
        for interval in [70, 120, 300] {
            let config = EXAMPLE_CONFIG.replace(
                "write_interval_s = 60",
                &format!("write_interval_s = {interval}"),
            );
            let error = Settings::parse(config).unwrap_err().to_string();
            assert!(error.contains("divide 60"), "{interval}: {error}");
        }
        let odd = EXAMPLE_CONFIG.replace("sample_interval_s = 2", "sample_interval_s = 7");
        assert!(
            Settings::parse(odd)
                .unwrap_err()
                .to_string()
                .contains("multiple of sample_interval_s")
        );
    }

    #[test]
    fn hardware_values_are_validated() {
        let psu = EXAMPLE_CONFIG.replace("psu_efficiency = 0.90", "psu_efficiency = 1.5");
        assert!(
            Settings::parse(psu)
                .unwrap_err()
                .to_string()
                .contains("psu_efficiency")
        );
        let provider = EXAMPLE_CONFIG.replace(
            "# providers = { amdgpu = false }",
            "providers = { intel_gpu = false }",
        );
        assert!(
            Settings::parse(provider)
                .unwrap_err()
                .to_string()
                .contains("unknown provider")
        );
    }

    #[test]
    fn unknown_keys_are_rejected() {
        let typo = EXAMPLE_CONFIG.replace("base_watts", "base_wats");
        assert!(matches!(Settings::parse(typo), Err(ConfigError::Parse(_))));
    }

    #[test]
    fn provider_switches() {
        let settings = Settings::parse(EXAMPLE_CONFIG.replace(
            "# providers = { amdgpu = false }",
            "providers = { amdgpu = false, cpu_estimate = true }",
        ))
        .unwrap();
        let hw = &settings.config.hardware;
        assert!(!hw.provider_enabled("amdgpu"));
        assert!(hw.provider_enabled("nvml"));
        assert!(hw.provider_forced("cpu_estimate"));
        assert!(!hw.provider_forced("nvml"));
    }

    #[test]
    fn missing_file_falls_back_to_example() {
        let dir = tempfile::tempdir().unwrap();
        let (settings, source) = load_or_example(&dir.path().join("absent.toml")).unwrap();
        assert_eq!(settings.text, EXAMPLE_CONFIG);
        assert!(matches!(source, ConfigSource::Example { .. }));
    }

    #[test]
    fn database_path_expands_home() {
        let config = CollectorConfig {
            database: Some(PathBuf::from("~/x/w.db")),
            ..CollectorConfig::default()
        };
        assert_eq!(
            config.database_path(),
            dirs::home_dir().unwrap().join("x/w.db")
        );
    }
}
