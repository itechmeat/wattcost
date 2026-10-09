//! Monitor power state: GNOME Mutter over D-Bus, with the DRM `dpms` attribute as a fallback.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use tracing::debug;
use zbus::blocking::{Proxy, connection, proxy::Builder};
use zbus::proxy::CacheProperties;

use super::sensor::{DisplaySensor, FsRoot, ProviderError};
use super::sysfs;

const DRM: &str = "sys/class/drm";
const MUTTER_SERVICE: &str = "org.gnome.Mutter.DisplayConfig";
const MUTTER_PATH: &str = "/org/gnome/Mutter/DisplayConfig";
const POWER_SAVE_ON: i32 = 0;
/// Well below the collector's gap threshold, so a hung desktop cannot stall sampling.
const CALL_TIMEOUT: Duration = Duration::from_secs(1);
const RETRY_AFTER: Duration = Duration::from_secs(30);

/// Spaces out attempts to reach a service that is not running.
#[derive(Debug, Default)]
struct Retry {
    next_attempt: Option<Instant>,
}

impl Retry {
    fn ready(&self, now: Instant) -> bool {
        self.next_attempt.is_none_or(|next| now >= next)
    }

    fn failed(&mut self, now: Instant) {
        self.next_attempt = Some(now + RETRY_AFTER);
    }

    fn succeeded(&mut self) {
        self.next_attempt = None;
    }
}

/// Connects lazily, so a collector started before the desktop session picks Mutter up later.
#[derive(Default)]
struct Mutter {
    proxy: Option<Proxy<'static>>,
    retry: Retry,
}

impl Mutter {
    fn power_save_mode(&mut self) -> zbus::Result<i32> {
        let now = Instant::now();
        if self.proxy.is_none() && !self.retry.ready(now) {
            return Err(zbus::Error::Failure(
                "Mutter was unavailable recently".to_owned(),
            ));
        }
        let result = self.read();
        match result {
            Ok(_) => self.retry.succeeded(),
            Err(_) => self.retry.failed(now),
        }
        result
    }

    fn read(&mut self) -> zbus::Result<i32> {
        let proxy = match self.proxy.take() {
            Some(proxy) => proxy,
            None => connect()?,
        };
        let mode = proxy.get_property::<i32>("PowerSaveMode")?;
        self.proxy = Some(proxy);
        Ok(mode)
    }
}

fn connect() -> zbus::Result<Proxy<'static>> {
    let connection = connection::Builder::session()?
        .method_timeout(CALL_TIMEOUT)
        .build()?;
    Builder::<Proxy<'static>>::new(&connection)
        .destination(MUTTER_SERVICE)?
        .path(MUTTER_PATH)?
        .interface(MUTTER_SERVICE)?
        .cache_properties(CacheProperties::No)
        .build()
}

pub struct DisplayProbe {
    mutter: Option<Mutter>,
    drm: PathBuf,
}

impl DisplayProbe {
    pub fn new(root: &FsRoot) -> Self {
        Self {
            mutter: Some(Mutter::default()),
            drm: root.join(DRM),
        }
    }

    #[cfg(test)]
    pub(crate) fn drm_only(root: &FsRoot) -> Self {
        Self {
            mutter: None,
            drm: root.join(DRM),
        }
    }

    /// Names the backend that answers now, for `wattcost detect`.
    pub fn describe(&mut self) -> Result<String, String> {
        if let Some(Ok(_)) = self.mutter.as_mut().map(Mutter::power_save_mode) {
            return Ok("GNOME Mutter PowerSaveMode".to_owned());
        }
        drm_is_on(&self.drm)
            .map(|_| "DRM dpms (with NVIDIA it may always read On)".to_owned())
            .map_err(|error| error.to_string())
    }
}

impl DisplaySensor for DisplayProbe {
    fn name(&self) -> &str {
        "display"
    }

    fn is_on(&mut self) -> Result<bool, ProviderError> {
        if let Some(mutter) = &mut self.mutter {
            match mutter.power_save_mode() {
                Ok(mode) => return Ok(mode == POWER_SAVE_ON),
                Err(error) => debug!("Mutter unavailable, using DRM: {error}"),
            }
        }
        drm_is_on(&self.drm)
    }
}

fn drm_is_on(drm: &std::path::Path) -> Result<bool, ProviderError> {
    let active: Vec<PathBuf> = sysfs::subdirs(drm)
        .into_iter()
        .filter(|output| sysfs::file_name(output).contains('-'))
        .filter(|output| {
            sysfs::read_trimmed(&output.join("status")).is_ok_and(|status| status == "connected")
        })
        .filter(|output| {
            sysfs::read_trimmed(&output.join("enabled")).is_ok_and(|enabled| enabled == "enabled")
        })
        .collect();
    if active.is_empty() {
        return Err(ProviderError::Device(
            "no connected display output".to_owned(),
        ));
    }
    Ok(active
        .iter()
        .any(|output| sysfs::read_trimmed(&output.join("dpms")).is_ok_and(|dpms| dpms == "On")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::FakeFs;

    fn output(fs: &FakeFs, name: &str, status: &str, enabled: &str, dpms: &str) {
        fs.write(&format!("{DRM}/{name}/status"), status);
        fs.write(&format!("{DRM}/{name}/enabled"), enabled);
        fs.write(&format!("{DRM}/{name}/dpms"), dpms);
    }

    #[test]
    fn on_when_any_active_output_is_on() {
        let fs = FakeFs::new();
        output(&fs, "card1-DP-1", "disconnected", "disabled", "On");
        output(&fs, "card1-DP-2", "connected", "enabled", "On");
        let mut probe = DisplayProbe::drm_only(&fs.root());
        assert!(probe.is_on().unwrap());
        output(&fs, "card1-DP-2", "connected", "enabled", "Off");
        assert!(!probe.is_on().unwrap());
    }

    #[test]
    fn no_connected_output_is_an_error() {
        let fs = FakeFs::new();
        output(&fs, "card0-HDMI-A-1", "disconnected", "disabled", "Off");
        let mut probe = DisplayProbe::drm_only(&fs.root());
        assert!(probe.is_on().is_err());
        assert!(probe.describe().is_err());
    }

    #[test]
    fn retry_waits_after_a_failure() {
        let start = Instant::now();
        let mut retry = Retry::default();
        assert!(retry.ready(start));
        retry.failed(start);
        assert!(!retry.ready(start + Duration::from_secs(29)));
        assert!(retry.ready(start + RETRY_AFTER));
        retry.succeeded();
        assert!(retry.ready(start));
    }
}
