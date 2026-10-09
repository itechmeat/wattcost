//! Whole-system power of a laptop running on battery.

use std::path::PathBuf;

use super::sensor::{Component, FsRoot, Provider, ProviderError, Quality, Reading};
use super::sysfs;

const POWER_SUPPLY: &str = "sys/class/power_supply";

struct Battery {
    name: String,
    dir: PathBuf,
}

pub(crate) fn probe(root: &FsRoot) -> Vec<Box<dyn Provider>> {
    sysfs::subdirs(&root.join(POWER_SUPPLY))
        .into_iter()
        .filter(|dir| sysfs::read_trimmed(&dir.join("type")).is_ok_and(|kind| kind == "Battery"))
        .map(|dir| {
            Box::new(Battery {
                name: format!("battery:{}", sysfs::file_name(&dir)),
                dir,
            }) as Box<dyn Provider>
        })
        .collect()
}

impl Battery {
    fn power_w(&self) -> Result<f64, ProviderError> {
        let power_now = self.dir.join("power_now");
        if power_now.exists() {
            return Ok(sysfs::read_number::<f64>(&power_now)? / 1e6);
        }
        let current_ua: f64 = sysfs::read_number(&self.dir.join("current_now"))?;
        let voltage_uv: f64 = sysfs::read_number(&self.dir.join("voltage_now"))?;
        Ok(current_ua * voltage_uv / 1e12)
    }
}

impl Provider for Battery {
    fn name(&self) -> &str {
        &self.name
    }

    fn component(&self) -> Component {
        Component::System
    }

    fn quality(&self) -> Quality {
        Quality::Reported
    }

    fn sample(&mut self, elapsed_s: f64) -> Result<Reading, ProviderError> {
        if sysfs::read_trimmed(&self.dir.join("status"))? != "Discharging" {
            return Ok(Reading::default());
        }
        let power_w = self.power_w()?;
        Ok(Reading {
            energy_j: Some(power_w * elapsed_s),
            power_w: Some(power_w),
            util_pct: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::FakeFs;

    fn battery(fs: &FakeFs, status: &str) {
        fs.write("sys/class/power_supply/BAT0/type", "Battery");
        fs.write("sys/class/power_supply/BAT0/status", status);
        fs.write("sys/class/power_supply/AC/type", "Mains");
    }

    #[test]
    fn reports_power_only_while_discharging() {
        let fs = FakeFs::new();
        battery(&fs, "Discharging");
        fs.write("sys/class/power_supply/BAT0/power_now", "12500000");
        let mut providers = probe(&fs.root());
        assert_eq!(providers.len(), 1);
        assert_eq!(providers[0].component(), Component::System);
        assert_eq!(providers[0].sample(2.0).unwrap().energy_j, Some(25.0));
        fs.write("sys/class/power_supply/BAT0/status", "Charging");
        assert_eq!(providers[0].sample(2.0).unwrap(), Reading::default());
    }

    #[test]
    fn derives_power_from_current_and_voltage() {
        let fs = FakeFs::new();
        battery(&fs, "Discharging");
        fs.write("sys/class/power_supply/BAT0/current_now", "1000000");
        fs.write("sys/class/power_supply/BAT0/voltage_now", "12000000");
        let mut providers = probe(&fs.root());
        assert_eq!(providers[0].sample(1.0).unwrap().power_w, Some(12.0));
    }

    #[test]
    fn desktop_has_no_battery() {
        let fs = FakeFs::new();
        fs.write("sys/class/power_supply/AC/type", "Mains");
        assert!(probe(&fs.root()).is_empty());
    }
}
