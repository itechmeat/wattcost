//! Converts measured component energy into energy drawn from the wall.

use crate::config::HardwareConfig;
use crate::sample::RawSample;

const JOULES_PER_WH: f64 = 3600.0;

/// Seconds of a row with the monitor on; an unknown display state follows `monitor_assumed_on`.
pub fn monitor_on_s(hardware: &HardwareConfig, sample: &RawSample) -> f64 {
    let assumed_on_s = if hardware.monitor_assumed_on {
        sample.duration_s
    } else {
        0.0
    };
    sample.display_on_s.unwrap_or(assumed_on_s)
}

/// Average power of `wall_wh` drawn over `seconds`; none without covered time.
pub fn average_watts(wall_wh: f64, seconds: f64) -> Option<f64> {
    (seconds > 0.0).then(|| wall_wh * JOULES_PER_WH / seconds)
}

/// Wall power for a given combined CPU and GPU power, on the same terms as [`wall_wh`].
pub fn wall_watts(hardware: &HardwareConfig, components_w: f64, monitor_on: bool) -> f64 {
    let monitor_w = if monitor_on {
        hardware.monitor_watts
    } else {
        0.0
    };
    (components_w + hardware.base_watts) / hardware.psu_efficiency + monitor_w
}

/// Wall energy of one row in watt-hours.
///
/// Whole-system energy (a laptop on battery) replaces the component estimate for the seconds it
/// covers. The monitor has its own power supply, so it is not divided by the PSU efficiency.
pub fn wall_wh(hardware: &HardwareConfig, sample: &RawSample) -> f64 {
    let on_mains_share = if sample.duration_s > 0.0 {
        (1.0 - sample.system_covered_s / sample.duration_s).clamp(0.0, 1.0)
    } else {
        1.0
    };
    let components_j = sample.cpu.energy_j.unwrap_or(0.0)
        + sample.gpu.energy_j.unwrap_or(0.0)
        + hardware.base_watts * sample.duration_s;
    let computer_j = sample.system.energy_j.unwrap_or(0.0)
        + components_j * on_mains_share / hardware.psu_efficiency;
    let monitor_j = hardware.monitor_watts * monitor_on_s(hardware, sample);
    (computer_j + monitor_j) / JOULES_PER_WH
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use super::*;
    use crate::config::HardwareConfig;
    use crate::sample::{ComponentStats, RawSample};

    fn sample(cpu_j: f64, gpu_j: f64, display_on_s: Option<f64>) -> RawSample {
        RawSample {
            start: Utc.with_ymd_and_hms(2026, 1, 15, 12, 0, 0).unwrap(),
            duration_s: 60.0,
            cpu: ComponentStats {
                energy_j: Some(cpu_j),
                ..ComponentStats::default()
            },
            gpu: ComponentStats {
                energy_j: Some(gpu_j),
                ..ComponentStats::default()
            },
            system: ComponentStats::default(),
            display_on_s,
            system_covered_s: 0.0,
            peak_power_w: None,
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

    #[test]
    fn components_go_through_the_psu_and_the_monitor_does_not() {
        let wh = wall_wh(&hardware(), &sample(5400.0, 19200.0, Some(30.0)));
        let expected = ((5400.0 + 19200.0 + 40.0 * 60.0) / 0.9 + 40.0 * 30.0) / 3600.0;
        assert!((wh - expected).abs() < 1e-9);
    }

    #[test]
    fn unknown_display_uses_assumption() {
        let mut hw = hardware();
        let on = wall_wh(&hw, &sample(0.0, 0.0, None));
        hw.monitor_assumed_on = false;
        let off = wall_wh(&hw, &sample(0.0, 0.0, None));
        assert!((on - off - 40.0 * 60.0 / 3600.0).abs() < 1e-9);
    }

    #[test]
    fn system_energy_replaces_components() {
        let mut s = sample(5400.0, 19200.0, Some(0.0));
        s.system.energy_j = Some(1800.0);
        s.system_covered_s = 60.0;
        assert!((wall_wh(&hardware(), &s) - 0.5).abs() < 1e-9);
    }

    #[test]
    fn partly_on_battery_combines_both_sources() {
        let mut s = sample(5400.0, 19200.0, Some(0.0));
        s.system.energy_j = Some(1800.0);
        s.system_covered_s = 30.0;
        let components_j = (5400.0 + 19200.0 + 40.0 * 60.0) * 0.5 / 0.9;
        assert!((wall_wh(&hardware(), &s) - (1800.0 + components_j) / 3600.0).abs() < 1e-9);
    }

    #[test]
    fn wall_watts_adds_base_psu_losses_and_monitor() {
        let watts = wall_watts(&hardware(), 200.0, true);
        assert!((watts - ((200.0 + 40.0) / 0.9 + 40.0)).abs() < 1e-9);
        assert!((wall_watts(&hardware(), 200.0, false) - (240.0 / 0.9)).abs() < 1e-9);
    }
}
