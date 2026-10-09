//! CPU package energy from the RAPL powercap interface (Intel and AMD Zen).

use std::io;
use std::path::PathBuf;

use super::counter::Counter;
use super::sensor::{
    Component, FsRoot, ProbeResult, Provider, ProviderError, Quality, Reading, average_power,
};
use super::sysfs;

pub(crate) const POWERCAP: &str = "sys/class/powercap";
const MICROJOULES_PER_JOULE: f64 = 1e6;

struct Zone {
    energy_path: PathBuf,
    counter: Counter,
}

struct Rapl {
    zones: Vec<Zone>,
}

pub(crate) fn probe(root: &FsRoot) -> ProbeResult {
    let mut zones = Vec::new();
    for dir in sysfs::subdirs(&root.join(POWERCAP)) {
        // Package zones are `intel-rapl:N`; `intel-rapl:N:M` are parts of a package.
        let name = sysfs::file_name(&dir);
        if !name.starts_with("intel-rapl:") || name.matches(':').count() != 1 {
            continue;
        }
        if !sysfs::read_trimmed(&dir.join("name")).is_ok_and(|zone| zone.starts_with("package")) {
            continue;
        }
        let range: u64 = sysfs::read_number(&dir.join("max_energy_range_uj"))
            .map_err(|error| error.to_string())?;
        let energy_path = dir.join("energy_uj");
        sysfs::read_number::<u64>(&energy_path).map_err(|error| with_permission_hint(&error))?;
        zones.push(Zone {
            energy_path,
            counter: Counter::wrapping_at(range),
        });
    }
    if zones.is_empty() {
        return Ok(Vec::new());
    }
    Ok(vec![Box::new(Rapl { zones })])
}

fn with_permission_hint(error: &ProviderError) -> String {
    match error {
        ProviderError::Io { source, .. } if source.kind() == io::ErrorKind::PermissionDenied => {
            format!("{error}; run `wattcost setup` for the udev rule that grants read access")
        }
        _ => error.to_string(),
    }
}

impl Provider for Rapl {
    fn name(&self) -> &str {
        "rapl"
    }

    fn component(&self) -> Component {
        Component::Cpu
    }

    fn quality(&self) -> Quality {
        Quality::Measured
    }

    fn sample(&mut self, elapsed_s: f64) -> Result<Reading, ProviderError> {
        let mut total_uj = Some(0_u64);
        for zone in &mut self.zones {
            let delta = zone.counter.delta(sysfs::read_number(&zone.energy_path)?);
            total_uj = total_uj.zip(delta).map(|(sum, value)| sum + value);
        }
        let energy_j = total_uj.map(|uj| uj as f64 / MICROJOULES_PER_JOULE);
        Ok(Reading {
            energy_j,
            power_w: energy_j.and_then(|joules| average_power(joules, elapsed_s)),
            util_pct: None,
        })
    }

    fn reset(&mut self) {
        for zone in &mut self.zones {
            zone.counter.reset();
        }
    }
}

#[cfg(test)]
mod tests {

    use super::*;
    use crate::test_support::FakeFs;

    fn package(fs: &FakeFs, zone: &str, name: &str, energy: u64) {
        fs.write(&format!("{POWERCAP}/{zone}/name"), name);
        fs.write(&format!("{POWERCAP}/{zone}/max_energy_range_uj"), "1000000");
        fs.write(&format!("{POWERCAP}/{zone}/energy_uj"), &energy.to_string());
    }

    #[test]
    fn sums_package_zones_and_skips_subzones() {
        let fs = FakeFs::new();
        package(&fs, "intel-rapl:0", "package-0", 100);
        package(&fs, "intel-rapl:1", "package-1", 100);
        package(&fs, "intel-rapl:0:0", "core", 100);
        package(&fs, "intel-rapl-mmio:0", "package-0", 100);
        let mut providers = probe(&fs.root()).unwrap();
        assert_eq!(providers.len(), 1);
        let rapl = &mut providers[0];
        assert_eq!(rapl.sample(0.0).unwrap().energy_j, None);
        fs.write(&format!("{POWERCAP}/intel-rapl:0/energy_uj"), "500100");
        fs.write(&format!("{POWERCAP}/intel-rapl:1/energy_uj"), "500100");
        let reading = rapl.sample(2.0).unwrap();
        assert_eq!(reading.energy_j, Some(1.0));
        assert_eq!(reading.power_w, Some(0.5));
    }

    #[test]
    fn wraps_at_max_energy_range() {
        let fs = FakeFs::new();
        package(&fs, "intel-rapl:0", "package-0", 999_000);
        let mut providers = probe(&fs.root()).unwrap();
        providers[0].sample(0.0).unwrap();
        fs.write(&format!("{POWERCAP}/intel-rapl:0/energy_uj"), "1000");
        assert_eq!(providers[0].sample(1.0).unwrap().energy_j, Some(0.002));
    }

    #[test]
    fn no_zones_means_no_provider() {
        let fs = FakeFs::new();
        fs.mkdir(POWERCAP);
        assert!(probe(&fs.root()).unwrap().is_empty());
    }

    #[test]
    fn permission_denied_mentions_setup() {
        let denied = ProviderError::Io {
            path: PathBuf::from("energy_uj"),
            source: io::Error::from(io::ErrorKind::PermissionDenied),
        };
        assert!(with_permission_hint(&denied).contains("wattcost setup"));
        let missing = ProviderError::Io {
            path: PathBuf::from("energy_uj"),
            source: io::Error::from(io::ErrorKind::NotFound),
        };
        assert!(!with_permission_hint(&missing).contains("wattcost setup"));
    }
}
