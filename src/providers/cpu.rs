//! CPU utilisation from `/proc/stat`, and a power estimate derived from it.

use std::path::{Path, PathBuf};

use super::sensor::{Component, FsRoot, ProbeResult, Provider, ProviderError, Quality, Reading};
use crate::config::HardwareConfig;

#[derive(Debug, Clone, Copy)]
struct CpuTimes {
    busy: u64,
    total: u64,
}

fn read_cpu_times(path: &Path) -> Result<CpuTimes, ProviderError> {
    let text = std::fs::read_to_string(path).map_err(|source| ProviderError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let parse_error = || ProviderError::Parse {
        path: path.to_path_buf(),
        value: text.lines().next().unwrap_or_default().to_owned(),
    };
    let line = text
        .lines()
        .find(|line| line.starts_with("cpu "))
        .ok_or_else(parse_error)?;
    let fields: Vec<u64> = line
        .split_whitespace()
        .skip(1)
        .take(8)
        .map(str::parse)
        .collect::<Result<_, _>>()
        .map_err(|_| parse_error())?;
    if fields.len() < 5 {
        return Err(parse_error());
    }
    let idle = fields[3] + fields[4];
    let total: u64 = fields.iter().sum();
    Ok(CpuTimes {
        busy: total - idle,
        total,
    })
}

#[derive(Debug)]
struct Utilization {
    path: PathBuf,
    last: Option<CpuTimes>,
}

impl Utilization {
    fn new(root: &FsRoot) -> Result<Self, String> {
        let path = root.join("proc/stat");
        read_cpu_times(&path).map_err(|error| error.to_string())?;
        Ok(Self { path, last: None })
    }

    /// Busy share in percent since the previous call.
    fn sample(&mut self) -> Result<Option<f64>, ProviderError> {
        let now = read_cpu_times(&self.path)?;
        let Some(previous) = self.last.replace(now) else {
            return Ok(None);
        };
        let total = now.total.saturating_sub(previous.total);
        if total == 0 {
            return Ok(None);
        }
        Ok(Some(
            100.0 * now.busy.saturating_sub(previous.busy) as f64 / total as f64,
        ))
    }

    fn reset(&mut self) {
        self.last = None;
    }
}

struct CpuStat {
    util: Utilization,
}

impl Provider for CpuStat {
    fn name(&self) -> &str {
        "cpu_stat"
    }

    fn component(&self) -> Component {
        Component::Cpu
    }

    fn quality(&self) -> Quality {
        Quality::Measured
    }

    fn sample(&mut self, _elapsed_s: f64) -> Result<Reading, ProviderError> {
        Ok(Reading {
            util_pct: self.util.sample()?,
            ..Reading::default()
        })
    }

    fn reset(&mut self) {
        self.util.reset();
    }

    fn measures_energy(&self) -> bool {
        false
    }
}

/// CPU power modelled as idle power plus utilisation times the idle-to-max range.
struct CpuEstimate {
    util: Utilization,
    idle_w: f64,
    max_w: f64,
}

impl Provider for CpuEstimate {
    fn name(&self) -> &str {
        "cpu_estimate"
    }

    fn component(&self) -> Component {
        Component::Cpu
    }

    fn quality(&self) -> Quality {
        Quality::Estimated
    }

    fn sample(&mut self, elapsed_s: f64) -> Result<Reading, ProviderError> {
        let power_w = self
            .util
            .sample()?
            .map(|util| self.idle_w + util / 100.0 * (self.max_w - self.idle_w));
        Ok(Reading {
            energy_j: power_w.map(|watts| watts * elapsed_s),
            power_w,
            util_pct: None,
        })
    }

    fn reset(&mut self) {
        self.util.reset();
    }
}

pub(crate) fn probe_stat(root: &FsRoot) -> ProbeResult {
    Ok(vec![Box::new(CpuStat {
        util: Utilization::new(root)?,
    })])
}

pub(crate) fn probe_estimate(root: &FsRoot, hardware: &HardwareConfig) -> ProbeResult {
    Ok(vec![Box::new(CpuEstimate {
        util: Utilization::new(root)?,
        idle_w: hardware.cpu_idle_watts,
        max_w: hardware.cpu_max_watts,
    })])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::HardwareConfig;
    use crate::test_support::FakeFs;

    fn stat(busy: u64, idle: u64) -> String {
        format!("cpu  {busy} 0 0 {idle} 0 0 0 0 0 0\ncpu0 1 2 3 4 5 6 7 8 9 10\n")
    }

    #[test]
    fn utilisation_from_two_snapshots() {
        let fs = FakeFs::new();
        fs.write("proc/stat", &stat(100, 900));
        let mut providers = probe_stat(&fs.root()).unwrap();
        let cpu = &mut providers[0];
        assert_eq!(cpu.sample(0.0).unwrap().util_pct, None);
        fs.write("proc/stat", &stat(175, 925));
        assert_eq!(cpu.sample(2.0).unwrap().util_pct, Some(75.0));
    }

    #[test]
    fn estimate_scales_between_idle_and_max() {
        let fs = FakeFs::new();
        fs.write("proc/stat", &stat(0, 0));
        let hardware = HardwareConfig {
            cpu_idle_watts: 20.0,
            cpu_max_watts: 120.0,
            ..HardwareConfig::default()
        };
        let mut providers = probe_estimate(&fs.root(), &hardware).unwrap();
        let cpu = &mut providers[0];
        assert_eq!(cpu.quality(), Quality::Estimated);
        cpu.sample(0.0).unwrap();
        fs.write("proc/stat", &stat(50, 50));
        let reading = cpu.sample(2.0).unwrap();
        assert_eq!(reading.power_w, Some(70.0));
        assert_eq!(reading.energy_j, Some(140.0));
        assert_eq!(reading.util_pct, None);
    }

    #[test]
    fn missing_proc_stat_is_reported() {
        let fs = FakeFs::new();
        assert!(probe_stat(&fs.root()).err().unwrap().contains("proc/stat"));
    }
}
