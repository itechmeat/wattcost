//! Hardware sensors behind one interface, and their detection.

mod amdgpu;
mod battery;
mod counter;
mod cpu;
mod display;
mod nvml;
mod rapl;
mod sensor;
mod sysfs;

pub use display::DisplayProbe;
pub use sensor::{Component, DisplaySensor, FsRoot, Provider, ProviderError, Quality, Reading};
pub(crate) use sensor::{ProbeResult, average_power};

use crate::config::HardwareConfig;

#[derive(Debug)]
pub struct Probe {
    pub provider: &'static str,
    pub status: ProbeStatus,
}

#[derive(Debug)]
pub enum ProbeStatus {
    Found(Vec<(String, Quality)>),
    Disabled,
    Unavailable(String),
}

#[derive(Default)]
pub struct Detected {
    pub providers: Vec<Box<dyn Provider>>,
    pub display: Option<DisplayProbe>,
    pub probes: Vec<Probe>,
}

pub fn detect(root: &FsRoot, hardware: &HardwareConfig) -> Detected {
    let mut detected = Detected::default();
    let estimate_forced = hardware.provider_forced("cpu_estimate");
    let has_rapl = if estimate_forced {
        detected.record(
            "rapl",
            ProbeStatus::Unavailable("replaced by the forced cpu_estimate".to_owned()),
        );
        false
    } else {
        detected.add("rapl", hardware, || rapl::probe(root))
    };
    detected.add("cpu_stat", hardware, || cpu::probe_stat(root));
    detected.add("nvml", hardware, nvml::probe);
    let include_package = !has_rapl && !estimate_forced;
    detected.add("amdgpu", hardware, || {
        Ok(amdgpu::probe(root, include_package))
    });
    if detected.measures(Component::Cpu) {
        detected.record(
            "cpu_estimate",
            ProbeStatus::Unavailable("not needed: CPU power is measured".to_owned()),
        );
    } else {
        detected.add("cpu_estimate", hardware, || {
            cpu::probe_estimate(root, hardware)
        });
    }
    detected.add("battery", hardware, || Ok(battery::probe(root)));
    if hardware.provider_enabled("display") {
        let mut display = DisplayProbe::new(root);
        let status = match display.describe() {
            Ok(backend) => ProbeStatus::Found(vec![(backend, Quality::Measured)]),
            Err(reason) => ProbeStatus::Unavailable(reason),
        };
        detected.record("display", status);
        detected.display = Some(display);
    } else {
        detected.record("display", ProbeStatus::Disabled);
    }
    detected
}

impl Probe {
    /// One `(quality, detail)` line per device found, or one line saying why there is none.
    pub fn summary(&self) -> Vec<(&'static str, String)> {
        match &self.status {
            ProbeStatus::Found(devices) => devices
                .iter()
                .map(|(name, quality)| (quality.label(), name.clone()))
                .collect(),
            ProbeStatus::Disabled => vec![("-", "disabled in config".to_owned())],
            ProbeStatus::Unavailable(reason) => vec![("-", reason.clone())],
        }
    }
}

impl Detected {
    fn measures(&self, component: Component) -> bool {
        self.providers
            .iter()
            .any(|provider| provider.component() == component && provider.measures_energy())
    }

    pub fn found(&self, provider: &str) -> bool {
        self.probes.iter().any(|probe| {
            probe.provider == provider && matches!(probe.status, ProbeStatus::Found(_))
        })
    }

    /// Runs one probe unless the config disables it; returns whether it found anything.
    fn add(
        &mut self,
        name: &'static str,
        hardware: &HardwareConfig,
        probe: impl FnOnce() -> ProbeResult,
    ) -> bool {
        if !hardware.provider_enabled(name) {
            self.record(name, ProbeStatus::Disabled);
            return false;
        }
        match probe() {
            Ok(found) if !found.is_empty() => {
                let names = found
                    .iter()
                    .map(|provider| (provider.name().to_owned(), provider.quality()))
                    .collect();
                self.providers.extend(found);
                self.record(name, ProbeStatus::Found(names));
                true
            }
            Ok(_) => {
                self.record(name, ProbeStatus::Unavailable("no device found".to_owned()));
                false
            }
            Err(reason) => {
                self.record(name, ProbeStatus::Unavailable(reason));
                false
            }
        }
    }

    fn record(&mut self, provider: &'static str, status: ProbeStatus) {
        self.probes.push(Probe { provider, status });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::HardwareConfig;
    use crate::test_support::FakeFs;

    /// Hardware settings without the sensors a test machine might really have.
    fn offline() -> HardwareConfig {
        let mut hardware = HardwareConfig::default();
        for name in ["nvml", "display"] {
            hardware.providers.insert(name.to_owned(), false);
        }
        hardware
    }

    fn status<'a>(detected: &'a Detected, name: &str) -> &'a ProbeStatus {
        &detected
            .probes
            .iter()
            .find(|probe| probe.provider == name)
            .unwrap()
            .status
    }

    #[test]
    fn estimate_replaces_missing_rapl() {
        let fs = FakeFs::new();
        fs.write("proc/stat", "cpu  1 0 0 1 0 0 0 0 0 0\n");
        let detected = detect(&fs.root(), &offline());
        let names: Vec<&str> = detected
            .providers
            .iter()
            .map(|provider| provider.name())
            .collect();
        assert_eq!(names, ["cpu_stat", "cpu_estimate"]);
    }

    #[test]
    fn rapl_makes_the_estimate_unnecessary() {
        let fs = FakeFs::new();
        fs.write("proc/stat", "cpu  1 0 0 1 0 0 0 0 0 0\n");
        fs.write("sys/class/powercap/intel-rapl:0/name", "package-0");
        fs.write(
            "sys/class/powercap/intel-rapl:0/max_energy_range_uj",
            "1000",
        );
        fs.write("sys/class/powercap/intel-rapl:0/energy_uj", "1");
        let detected = detect(&fs.root(), &offline());
        let names: Vec<&str> = detected
            .providers
            .iter()
            .map(|provider| provider.name())
            .collect();
        assert_eq!(names, ["rapl", "cpu_stat"]);
        assert!(
            matches!(status(&detected, "rapl"), ProbeStatus::Found(found) if found[0].1 == Quality::Measured)
        );
    }

    #[test]
    fn disabled_providers_are_not_probed() {
        let fs = FakeFs::new();
        fs.write("proc/stat", "cpu  1 0 0 1 0 0 0 0 0 0\n");
        let mut hardware = offline();
        hardware.providers.insert("cpu_stat".to_owned(), false);
        let detected = detect(&fs.root(), &hardware);
        assert!(matches!(
            status(&detected, "cpu_stat"),
            ProbeStatus::Disabled
        ));
        assert!(detected.display.is_none());
    }

    #[test]
    fn integrated_gpu_package_power_replaces_the_estimate() {
        let fs = FakeFs::new();
        fs.write("proc/stat", "cpu  1 0 0 1 0 0 0 0 0 0\n");
        fs.write("proc/cpuinfo", "vendor_id\t: AuthenticAMD\n");
        fs.write(
            "sys/class/drm/card0/device/mem_info_vram_total",
            "536870912",
        );
        fs.write("sys/class/drm/card0/device/hwmon/hwmon5/name", "amdgpu");
        fs.write(
            "sys/class/drm/card0/device/hwmon/hwmon5/power1_input",
            "57000000",
        );
        let detected = detect(&fs.root(), &offline());
        let names: Vec<&str> = detected
            .providers
            .iter()
            .map(|provider| provider.name())
            .collect();
        assert_eq!(names, ["cpu_stat", "amdgpu-ppt:card0"]);
    }

    #[test]
    fn forced_estimate_replaces_measured_cpu_power() {
        let fs = FakeFs::new();
        fs.write("proc/stat", "cpu  1 0 0 1 0 0 0 0 0 0\n");
        fs.write("sys/class/powercap/intel-rapl:0/name", "package-0");
        fs.write(
            "sys/class/powercap/intel-rapl:0/max_energy_range_uj",
            "1000",
        );
        fs.write("sys/class/powercap/intel-rapl:0/energy_uj", "1");
        let mut hardware = offline();
        hardware.providers.insert("cpu_estimate".to_owned(), true);
        let detected = detect(&fs.root(), &hardware);
        let names: Vec<&str> = detected
            .providers
            .iter()
            .map(|provider| provider.name())
            .collect();
        assert_eq!(names, ["cpu_stat", "cpu_estimate"]);
        assert!(matches!(
            status(&detected, "rapl"),
            ProbeStatus::Unavailable(_)
        ));
    }
}
