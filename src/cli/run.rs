//! The foreground collection loop: sampling, recording, signals and config reload.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime};
use std::{fs, thread};

use anyhow::Context;
use chrono::{DateTime, Local, TimeZone, Utc};
use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};
use tracing::{error, info, warn};

use crate::collector::Collector;
use crate::config::{self, Settings};
use crate::pricing::{self, Pricing};
use crate::providers::{self, DisplaySensor, FsRoot};
use crate::sample::RawSample;
use crate::store::{Store, StoreError};

const POLL: Duration = Duration::from_millis(250);
/// One day of rows at the default interval.
const MAX_BACKLOG: usize = 24 * 60;

/// Rows that could not be stored yet (database locked, disk full), oldest first.
#[derive(Debug)]
struct Backlog<T> {
    rows: VecDeque<T>,
}

impl<T> Default for Backlog<T> {
    fn default() -> Self {
        Self {
            rows: VecDeque::new(),
        }
    }
}

impl<T> Backlog<T> {
    fn push(&mut self, row: T) {
        if self.rows.len() == MAX_BACKLOG {
            self.rows.pop_front();
        }
        self.rows.push_back(row);
    }

    /// Stores rows in order and stops at the first failure, keeping the rest for later.
    fn flush<E>(&mut self, mut store: impl FnMut(&T) -> Result<(), E>) -> Result<(), E> {
        while let Some(row) = self.rows.front() {
            store(row)?;
            self.rows.pop_front();
        }
        Ok(())
    }
}

/// Prices rows with the settings in force and stores them.
pub(crate) struct Recorder<Tz: TimeZone> {
    store: Store,
    settings: Settings,
    settings_id: i64,
    tz: Tz,
    backlog: Backlog<(RawSample, Pricing, i64)>,
}

impl<Tz: TimeZone> Recorder<Tz> {
    pub(crate) fn new(store: Store, settings: Settings, tz: Tz) -> Result<Self, StoreError> {
        let settings_id = store.settings_id(&settings.text, Utc::now())?;
        Ok(Self {
            store,
            settings,
            settings_id,
            tz,
            backlog: Backlog::default(),
        })
    }

    pub(crate) fn record(&mut self, sample: &RawSample) -> Result<(), StoreError> {
        let pricing = pricing::price(&self.settings, sample, &self.tz);
        self.backlog
            .push((sample.clone(), pricing, self.settings_id));
        let store = &self.store;
        self.backlog.flush(|(sample, pricing, settings_id)| {
            store.insert_sample(sample, pricing, *settings_id)
        })
    }

    /// Applies hardware constants and the tariff from `path`; keeps the current settings on error.
    pub(crate) fn reload(&mut self, path: &Path) -> anyhow::Result<()> {
        let settings = Settings::load(path)?;
        let (new, old) = (&settings.config, &self.settings.config);
        if new.collector != old.collector
            || new.hardware.providers != old.hardware.providers
            || new.hardware.cpu_idle_watts != old.hardware.cpu_idle_watts
            || new.hardware.cpu_max_watts != old.hardware.cpu_max_watts
        {
            warn!("collector, provider and CPU estimate changes apply after a restart");
        }
        self.settings_id = self.store.settings_id(&settings.text, Utc::now())?;
        self.settings = settings;
        info!("config reloaded");
        Ok(())
    }
}

pub(crate) fn next_tick(now: DateTime<Utc>, interval_s: u32) -> DateTime<Utc> {
    let step = i64::from(interval_s) * 1000;
    DateTime::from_timestamp_millis((now.timestamp_millis() / step + 1) * step)
        .expect("next tick is within range")
}

struct ConfigWatcher {
    path: PathBuf,
    modified: Option<SystemTime>,
}

impl ConfigWatcher {
    fn new(path: &Path) -> Self {
        Self {
            path: path.to_path_buf(),
            modified: modified(path),
        }
    }

    fn changed(&mut self) -> bool {
        let current = modified(&self.path);
        current != std::mem::replace(&mut self.modified, current)
    }
}

fn modified(path: &Path) -> Option<SystemTime> {
    fs::metadata(path).and_then(|meta| meta.modified()).ok()
}

pub(crate) fn run(config_path: &Path) -> anyhow::Result<()> {
    let (settings, source) = config::load_or_example(config_path)?;
    if let config::ConfigSource::Example { missing } = &source {
        warn!(
            "no config at {}; using the example values",
            missing.display()
        );
    }
    let detected = providers::detect(&FsRoot::default(), &settings.config.hardware);
    for probe in &detected.probes {
        for (quality, detail) in probe.summary() {
            info!(provider = probe.provider, "{quality}: {detail}");
        }
    }
    let database = settings.config.collector.database_path();
    let store =
        Store::open(&database).with_context(|| format!("opening {}", database.display()))?;
    let sample_interval_s = settings.config.collector.sample_interval_s;
    let interval = Duration::from_secs(u64::from(sample_interval_s));
    let display = detected
        .display
        .map(|display| Box::new(display) as Box<dyn DisplaySensor>);
    let mut collector = Collector::new(detected.providers, display, &settings.config.collector);
    let mut recorder = Recorder::new(store, settings, Local)?;
    let mut watcher = ConfigWatcher::new(config_path);

    let stop = Arc::new(AtomicBool::new(false));
    let reload = Arc::new(AtomicBool::new(false));
    for signal in [SIGTERM, SIGINT] {
        signal_hook::flag::register(signal, Arc::clone(&stop))?;
    }
    signal_hook::flag::register(SIGHUP, Arc::clone(&reload))?;
    info!("collecting into {}", database.display());

    loop {
        if let Some(sample) = collector.tick(Utc::now()) {
            record(&mut recorder, &sample);
        }
        let signalled = reload.swap(false, Ordering::Relaxed);
        let edited = watcher.changed();
        if signalled || edited {
            if let Err(error) = recorder.reload(config_path) {
                warn!("keeping the previous config: {error:#}");
            }
        }
        let now = Utc::now();
        if !sleep_for(
            wait_for(now, next_tick(now, sample_interval_s), interval),
            &stop,
        ) {
            break;
        }
    }
    if let Some(sample) = collector.flush() {
        record(&mut recorder, &sample);
    }
    info!("stopped");
    Ok(())
}

fn record<Tz: TimeZone>(recorder: &mut Recorder<Tz>, sample: &RawSample) {
    if let Err(error) = recorder.record(sample) {
        error!("cannot store a row: {error}");
    }
}

/// Time until the next aligned tick, capped at one interval so a wall clock set back cannot stall
/// sampling; the collector then sees the step back as a gap.
fn wait_for(now: DateTime<Utc>, deadline: DateTime<Utc>, interval: Duration) -> Duration {
    (deadline - now)
        .to_std()
        .unwrap_or(Duration::ZERO)
        .min(interval)
}

/// Sleeps in short steps of monotonic time so signals are noticed; false on stop.
fn sleep_for(wait: Duration, stop: &AtomicBool) -> bool {
    let start = Instant::now();
    loop {
        if stop.load(Ordering::Relaxed) {
            return false;
        }
        let left = wait.saturating_sub(start.elapsed());
        if left.is_zero() {
            return true;
        }
        thread::sleep(left.min(POLL));
    }
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use super::*;
    use crate::config::{EXAMPLE_CONFIG, Settings};
    use crate::sample::{ComponentStats, RawSample};
    use crate::test_support::TempStore;

    fn sample() -> RawSample {
        RawSample {
            start: Utc.with_ymd_and_hms(2026, 1, 15, 12, 0, 0).unwrap(),
            duration_s: 60.0,
            cpu: ComponentStats::default(),
            gpu: ComponentStats::default(),
            system: ComponentStats {
                energy_j: Some(3600.0),
                ..ComponentStats::default()
            },
            display_on_s: Some(0.0),
            system_covered_s: 60.0,
            peak_power_w: None,
        }
    }

    #[test]
    fn invalid_config_keeps_previous_settings() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "not = [valid").unwrap();
        let temp = TempStore::new();
        let mut recorder = Recorder::new(temp.open(), Settings::example(), Utc).unwrap();
        assert!(recorder.reload(&path).is_err());
        recorder.record(&sample()).unwrap();
        let rows = temp
            .open()
            .samples(
                sample().start,
                sample().start + chrono::TimeDelta::minutes(1),
            )
            .unwrap();
        assert_eq!(rows[0].pricing.currency, "EUR");
    }

    #[test]
    fn valid_reload_prices_new_rows_with_new_settings() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            EXAMPLE_CONFIG.replace("currency = \"EUR\"", "currency = \"USD\""),
        )
        .unwrap();
        let temp = TempStore::new();
        let mut recorder = Recorder::new(temp.open(), Settings::example(), Utc).unwrap();
        recorder.reload(&path).unwrap();
        recorder.record(&sample()).unwrap();
        let rows = temp
            .open()
            .samples(
                sample().start,
                sample().start + chrono::TimeDelta::minutes(1),
            )
            .unwrap();
        assert_eq!(rows[0].pricing.currency, "USD");
    }

    #[test]
    fn ticks_align_to_the_interval() {
        let now = Utc.with_ymd_and_hms(2026, 1, 15, 12, 0, 3).unwrap()
            + chrono::TimeDelta::milliseconds(250);
        assert_eq!(
            next_tick(now, 2),
            Utc.with_ymd_and_hms(2026, 1, 15, 12, 0, 4).unwrap()
        );
    }

    #[test]
    fn wait_is_capped_at_one_interval() {
        let now = Utc.with_ymd_and_hms(2026, 1, 15, 12, 0, 0).unwrap();
        let interval = Duration::from_secs(2);
        assert_eq!(
            wait_for(now, now + chrono::TimeDelta::hours(1), interval),
            interval
        );
        assert_eq!(
            wait_for(now, now - chrono::TimeDelta::seconds(1), interval),
            Duration::ZERO
        );
        assert_eq!(
            wait_for(now, now + chrono::TimeDelta::milliseconds(500), interval),
            Duration::from_millis(500)
        );
    }

    #[test]
    fn failed_rows_are_retried_in_order() {
        let mut backlog = Backlog::default();
        backlog.push(1);
        backlog.push(2);
        assert!(backlog.flush(|_| Err::<(), _>("locked")).is_err());
        let mut stored = Vec::new();
        backlog
            .flush(|row| {
                stored.push(*row);
                Ok::<(), &str>(())
            })
            .unwrap();
        assert_eq!(stored, [1, 2]);
        assert!(backlog.flush(|_| Err::<(), _>("never called")).is_ok());
    }

    #[test]
    fn backlog_drops_the_oldest_rows_when_full() {
        let mut backlog = Backlog::default();
        for row in 0..=MAX_BACKLOG {
            backlog.push(row);
        }
        let mut first = None;
        backlog
            .flush(|row| {
                first.get_or_insert(*row);
                Ok::<(), ()>(())
            })
            .unwrap();
        assert_eq!(first, Some(1));
    }

    #[test]
    fn a_saved_config_is_noticed_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, EXAMPLE_CONFIG).unwrap();
        let mut watcher = ConfigWatcher::new(&path);
        assert!(!watcher.changed());
        crate::settings_form::write_atomically(&path, EXAMPLE_CONFIG).unwrap();
        // Coarse file-system timestamps could hide a write made in the same tick.
        let later = SystemTime::now() + Duration::from_secs(5);
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(later)
            .unwrap();
        assert!(watcher.changed());
        assert!(!watcher.changed());
    }
}
