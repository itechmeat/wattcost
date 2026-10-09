//! The interface every sensor implements, and the values it produces.

use std::io;
use std::path::PathBuf;

use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Component {
    Cpu,
    Gpu,
    System,
}

/// What one provider saw during one sampling interval.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Reading {
    pub energy_j: Option<f64>,
    pub power_w: Option<f64>,
    pub util_pct: Option<f64>,
}

#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("{path}: cannot parse {value:?}")]
    Parse { path: PathBuf, value: String },
    #[error("{0}")]
    Device(String),
}

pub trait Provider {
    fn name(&self) -> &str;
    fn component(&self) -> Component;
    fn quality(&self) -> Quality;
    /// Reads the sensor; `elapsed_s` is the time since the previous call. The first call after
    /// construction or [`Provider::reset`] only establishes a baseline for energy counters.
    fn sample(&mut self, elapsed_s: f64) -> Result<Reading, ProviderError>;
    fn reset(&mut self) {}
    /// False for providers that only report utilisation.
    fn measures_energy(&self) -> bool {
        true
    }
}

pub trait DisplaySensor {
    fn name(&self) -> &str;
    fn is_on(&mut self) -> Result<bool, ProviderError>;
}

/// Root of the `/sys` and `/proc` trees, replaceable in tests.
#[derive(Debug, Clone)]
pub struct FsRoot(PathBuf);

impl FsRoot {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self(path.into())
    }

    pub fn join(&self, relative: &str) -> PathBuf {
        self.0.join(relative)
    }
}

impl Default for FsRoot {
    fn default() -> Self {
        Self::new("/")
    }
}

pub(crate) type ProbeResult = Result<Vec<Box<dyn Provider>>, String>;

pub(crate) fn average_power(energy_j: f64, elapsed_s: f64) -> Option<f64> {
    (elapsed_s > 0.0).then(|| energy_j / elapsed_s)
}

/// How a value was obtained, from best to worst: an energy counter, a power reading, a model.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Quality {
    #[default]
    Missing,
    Estimated,
    Reported,
    Measured,
}

impl Quality {
    pub fn code(self) -> i64 {
        match self {
            Self::Missing => 0,
            Self::Estimated => 1,
            Self::Reported => 2,
            Self::Measured => 3,
        }
    }

    pub fn from_code(code: i64) -> Self {
        match code {
            1 => Self::Estimated,
            2 => Self::Reported,
            3 => Self::Measured,
            _ => Self::Missing,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::Estimated => "estimated",
            Self::Reported => "reported",
            Self::Measured => "measured",
        }
    }
}
