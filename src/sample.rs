//! One aggregated row of raw measurements.

use chrono::{DateTime, Utc};

use crate::providers::Quality;

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ComponentStats {
    pub util_avg: Option<f64>,
    pub util_max: Option<f64>,
    pub energy_j: Option<f64>,
    pub power_max_w: Option<f64>,
    pub quality: Quality,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct RawSample {
    pub start: DateTime<Utc>,
    pub duration_s: f64,
    pub cpu: ComponentStats,
    pub gpu: ComponentStats,
    pub system: ComponentStats,
    pub display_on_s: Option<f64>,
    /// Seconds of the row for which `system` supplied whole-system energy.
    pub system_covered_s: f64,
    /// Highest combined CPU and GPU power seen in one sampling interval.
    pub peak_power_w: Option<f64>,
}
