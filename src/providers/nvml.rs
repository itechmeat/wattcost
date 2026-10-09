//! NVIDIA GPUs through NVML, loaded at runtime so the binary works without the driver.

use std::rc::Rc;

use nvml_wrapper::Nvml;
use nvml_wrapper::error::NvmlError;

use super::counter::Counter;
use super::sensor::{Component, ProbeResult, Provider, ProviderError, Quality, Reading};

const MILLI: f64 = 1000.0;

enum EnergySource {
    /// Total energy counter (Volta and newer).
    Counter(Counter),
    /// Older GPUs: integrate the power reading.
    PowerReadings,
}

struct NvidiaGpu {
    nvml: Rc<Nvml>,
    index: u32,
    name: String,
    energy: EnergySource,
}

pub(crate) fn probe() -> ProbeResult {
    let nvml = Rc::new(
        Nvml::init()
            .map_err(|error| format!("NVIDIA management library not available: {error}"))?,
    );
    let count = nvml.device_count().map_err(|error| error.to_string())?;
    let mut gpus: Vec<Box<dyn Provider>> = Vec::new();
    for index in 0..count {
        let device = nvml
            .device_by_index(index)
            .map_err(|error| error.to_string())?;
        let model = device.name().unwrap_or_else(|_| "NVIDIA GPU".to_owned());
        let energy = if device.total_energy_consumption().is_ok() {
            EnergySource::Counter(Counter::monotonic())
        } else {
            EnergySource::PowerReadings
        };
        gpus.push(Box::new(NvidiaGpu {
            nvml: Rc::clone(&nvml),
            index,
            name: format!("nvml:{index} {model}"),
            energy,
        }));
    }
    Ok(gpus)
}

impl From<NvmlError> for ProviderError {
    fn from(error: NvmlError) -> Self {
        Self::Device(format!("NVML: {error}"))
    }
}

impl Provider for NvidiaGpu {
    fn name(&self) -> &str {
        &self.name
    }

    fn component(&self) -> Component {
        Component::Gpu
    }

    fn quality(&self) -> Quality {
        match self.energy {
            EnergySource::Counter(_) => Quality::Measured,
            EnergySource::PowerReadings => Quality::Reported,
        }
    }

    fn sample(&mut self, elapsed_s: f64) -> Result<Reading, ProviderError> {
        let device = self.nvml.device_by_index(self.index)?;
        let power_w = device
            .power_usage()
            .ok()
            .map(|milliwatts| f64::from(milliwatts) / MILLI);
        let util_pct = device
            .utilization_rates()
            .ok()
            .map(|rates| f64::from(rates.gpu));
        let energy_j = match &mut self.energy {
            EnergySource::Counter(counter) => counter
                .delta(device.total_energy_consumption()?)
                .map(|millijoules| millijoules as f64 / MILLI),
            EnergySource::PowerReadings => power_w.map(|watts| watts * elapsed_s),
        };
        Ok(Reading {
            energy_j,
            power_w,
            util_pct,
        })
    }

    fn reset(&mut self) {
        if let EnergySource::Counter(counter) = &mut self.energy {
            counter.reset();
        }
    }
}
