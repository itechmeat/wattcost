//! AMD GPU power and utilisation from the amdgpu hwmon interface.
//!
//! On an integrated GPU the same sensor reports the whole CPU package (PPT), so it is exposed
//! as CPU power instead of GPU power.

use std::path::{Path, PathBuf};

use super::sensor::{Component, FsRoot, Provider, ProviderError, Quality, Reading};
use super::sysfs;

const DRM: &str = "sys/class/drm";
const MICROWATTS_PER_WATT: f64 = 1e6;
/// Integrated GPUs get a small carve-out; discrete cards have far more VRAM.
const MAX_INTEGRATED_VRAM_BYTES: u64 = 2 << 30;

struct AmdGpu {
    name: String,
    component: Component,
    power_path: PathBuf,
    busy_path: Option<PathBuf>,
}

/// `include_package` adds integrated GPUs as CPU package power; pass false when RAPL measures the CPU.
pub(crate) fn probe(root: &FsRoot, include_package: bool) -> Vec<Box<dyn Provider>> {
    let amd_cpu = sysfs::read_trimmed(&root.join("proc/cpuinfo"))
        .is_ok_and(|cpuinfo| cpuinfo.contains("AuthenticAMD"));
    let mut gpus: Vec<Box<dyn Provider>> = Vec::new();
    for card in sysfs::subdirs(&root.join(DRM)) {
        let card_name = sysfs::file_name(&card);
        if !card_name.starts_with("card") || card_name.contains('-') {
            continue;
        }
        let device = card.join("device");
        let Some(hwmon) = sysfs::subdirs(&device.join("hwmon"))
            .into_iter()
            .find(|hwmon| {
                sysfs::read_trimmed(&hwmon.join("name")).is_ok_and(|name| name == "amdgpu")
            })
        else {
            continue;
        };
        let Some(power_path) = ["power1_average", "power1_input"]
            .iter()
            .map(|file| hwmon.join(file))
            .find(|path| path.exists())
        else {
            continue;
        };
        let integrated = amd_cpu && is_integrated(&device);
        if integrated && !include_package {
            continue;
        }
        let (name, component, busy_path) = if integrated {
            (format!("amdgpu-ppt:{card_name}"), Component::Cpu, None)
        } else {
            let busy = Some(device.join("gpu_busy_percent")).filter(|path| path.exists());
            (format!("amdgpu:{card_name}"), Component::Gpu, busy)
        };
        gpus.push(Box::new(AmdGpu {
            name,
            component,
            power_path,
            busy_path,
        }));
    }
    gpus
}

/// Discrete cards report a VRAM vendor and carry gigabytes of VRAM; both must point to an iGPU.
fn is_integrated(device: &Path) -> bool {
    !device.join("mem_info_vram_vendor").exists()
        && sysfs::read_number::<u64>(&device.join("mem_info_vram_total"))
            .is_ok_and(|bytes| bytes <= MAX_INTEGRATED_VRAM_BYTES)
}

impl Provider for AmdGpu {
    fn name(&self) -> &str {
        &self.name
    }

    fn component(&self) -> Component {
        self.component
    }

    fn quality(&self) -> Quality {
        Quality::Reported
    }

    fn sample(&mut self, elapsed_s: f64) -> Result<Reading, ProviderError> {
        let power_w = sysfs::read_number::<f64>(&self.power_path)? / MICROWATTS_PER_WATT;
        let util_pct = self
            .busy_path
            .as_deref()
            .map(sysfs::read_number::<f64>)
            .transpose()?;
        Ok(Reading {
            energy_j: Some(power_w * elapsed_s),
            power_w: Some(power_w),
            util_pct,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::FakeFs;

    #[test]
    fn reads_power_and_busy_percent() {
        let fs = FakeFs::new();
        fs.write("sys/class/drm/card1/device/hwmon/hwmon3/name", "amdgpu");
        fs.write(
            "sys/class/drm/card1/device/hwmon/hwmon3/power1_average",
            "45000000",
        );
        fs.write("sys/class/drm/card1/device/gpu_busy_percent", "30");
        fs.write("sys/class/drm/card1/device/mem_info_vram_vendor", "samsung");
        fs.write("sys/class/drm/card1-DP-1/status", "connected");
        fs.write("sys/class/drm/card0/device/hwmon/hwmon2/name", "nvidia");
        let mut providers = probe(&fs.root(), true);
        assert_eq!(providers.len(), 1);
        assert_eq!(providers[0].name(), "amdgpu:card1");
        let reading = providers[0].sample(2.0).unwrap();
        assert_eq!(reading.power_w, Some(45.0));
        assert_eq!(reading.energy_j, Some(90.0));
        assert_eq!(reading.util_pct, Some(30.0));
    }

    #[test]
    fn falls_back_to_power1_input() {
        let fs = FakeFs::new();
        fs.write("sys/class/drm/card0/device/hwmon/hwmon1/name", "amdgpu");
        fs.write(
            "sys/class/drm/card0/device/hwmon/hwmon1/power1_input",
            "10000000",
        );
        let mut providers = probe(&fs.root(), true);
        assert_eq!(providers[0].sample(1.0).unwrap().power_w, Some(10.0));
        assert_eq!(providers[0].sample(1.0).unwrap().util_pct, None);
    }

    fn integrated(fs: &FakeFs) {
        fs.write(
            "proc/cpuinfo",
            "processor\t: 0\nvendor_id\t: AuthenticAMD\n",
        );
        fs.write(
            "sys/class/drm/card0/device/mem_info_vram_total",
            "536870912",
        );
    }

    #[test]
    fn integrated_gpu_reports_cpu_package_power() {
        let fs = FakeFs::new();
        integrated(&fs);
        fs.write("sys/class/drm/card0/device/hwmon/hwmon5/name", "amdgpu");
        fs.write(
            "sys/class/drm/card0/device/hwmon/hwmon5/power1_input",
            "57000000",
        );
        fs.write("sys/class/drm/card0/device/gpu_busy_percent", "0");
        let mut providers = probe(&fs.root(), true);
        assert_eq!(providers[0].name(), "amdgpu-ppt:card0");
        assert_eq!(providers[0].component(), Component::Cpu);
        assert_eq!(providers[0].sample(1.0).unwrap().util_pct, None);
        assert!(probe(&fs.root(), false).is_empty());
    }

    #[test]
    fn discrete_card_without_vram_vendor_stays_a_gpu() {
        let fs = FakeFs::new();
        integrated(&fs);
        fs.write(
            "sys/class/drm/card0/device/mem_info_vram_total",
            "17163091968",
        );
        fs.write("sys/class/drm/card0/device/hwmon/hwmon1/name", "amdgpu");
        fs.write(
            "sys/class/drm/card0/device/hwmon/hwmon1/power1_average",
            "200000000",
        );
        let providers = probe(&fs.root(), true);
        assert_eq!(providers[0].name(), "amdgpu:card0");
        assert_eq!(providers[0].component(), Component::Gpu);
    }

    #[test]
    fn small_vram_without_an_amd_cpu_stays_a_gpu() {
        let fs = FakeFs::new();
        fs.write(
            "proc/cpuinfo",
            "processor\t: 0\nvendor_id\t: GenuineIntel\n",
        );
        fs.write(
            "sys/class/drm/card0/device/mem_info_vram_total",
            "536870912",
        );
        fs.write("sys/class/drm/card0/device/hwmon/hwmon1/name", "amdgpu");
        fs.write(
            "sys/class/drm/card0/device/hwmon/hwmon1/power1_input",
            "30000000",
        );
        assert_eq!(probe(&fs.root(), true)[0].component(), Component::Gpu);
    }
}
