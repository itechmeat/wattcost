//! Turns provider readings taken every few seconds into one row per write interval.

use std::collections::HashSet;

use chrono::{DateTime, Utc};
use tracing::{info, warn};

use crate::config::CollectorConfig;
use crate::providers::{Component, DisplaySensor, Provider, Quality, Reading, average_power};
use crate::sample::{ComponentStats, RawSample};

/// A pause longer than this many sample intervals is treated as a gap (suspend, stopped service).
const GAP_INTERVALS: f64 = 3.0;
const DISPLAY: &str = "display";

pub struct Collector {
    providers: Vec<Box<dyn Provider>>,
    display: Option<Box<dyn DisplaySensor>>,
    write_interval_s: i64,
    gap_s: f64,
    last_tick: Option<DateTime<Utc>>,
    row: Option<RowBuilder>,
    failing: HashSet<String>,
}

impl Collector {
    pub fn new(
        providers: Vec<Box<dyn Provider>>,
        display: Option<Box<dyn DisplaySensor>>,
        config: &CollectorConfig,
    ) -> Self {
        Self {
            providers,
            display,
            write_interval_s: i64::from(config.write_interval_s),
            gap_s: GAP_INTERVALS * f64::from(config.sample_interval_s),
            last_tick: None,
            row: None,
            failing: HashSet::new(),
        }
    }

    /// Samples every sensor at `now`; returns a row when `now` completes one.
    pub fn tick(&mut self, now: DateTime<Utc>) -> Option<RawSample> {
        let Some(previous) = self.last_tick.replace(now) else {
            self.read_providers(0.0);
            return None;
        };
        let elapsed_s = (now - previous).as_seconds_f64();
        if elapsed_s <= 0.0 || elapsed_s > self.gap_s {
            self.rebaseline();
            return self.flush();
        }
        let bucket = self.bucket_start(previous);
        let finished = if self.row.as_ref().is_some_and(|row| row.start != bucket) {
            self.flush()
        } else {
            None
        };
        let readings = self.read_providers(elapsed_s);
        let display_on = self.read_display();
        self.row
            .get_or_insert_with(|| RowBuilder::new(bucket))
            .add(elapsed_s, &readings, display_on);
        finished
    }

    pub fn flush(&mut self) -> Option<RawSample> {
        self.row.take().map(RowBuilder::finish)
    }

    fn rebaseline(&mut self) {
        for provider in &mut self.providers {
            provider.reset();
        }
        self.read_providers(0.0);
    }

    fn bucket_start(&self, time: DateTime<Utc>) -> DateTime<Utc> {
        let seconds = time.timestamp();
        DateTime::from_timestamp(seconds - seconds.rem_euclid(self.write_interval_s), 0)
            .expect("a bucket start is within chrono's range")
    }

    fn read_providers(&mut self, elapsed_s: f64) -> Vec<(Component, Quality, Reading)> {
        let mut readings = Vec::with_capacity(self.providers.len());
        for provider in &mut self.providers {
            let result = provider.sample(elapsed_s);
            track(&mut self.failing, provider.name(), result.as_ref().err());
            match result {
                Ok(reading) => readings.push((provider.component(), provider.quality(), reading)),
                Err(_) => {
                    // A counter read after an outage would otherwise put the whole outage into one interval.
                    provider.reset();
                    readings.push((provider.component(), Quality::Missing, Reading::default()));
                }
            }
        }
        readings
    }

    fn read_display(&mut self) -> Option<bool> {
        let display = self.display.as_mut()?;
        let result = display.is_on();
        track(&mut self.failing, DISPLAY, result.as_ref().err());
        result.ok()
    }
}

/// Logs each failure once and each recovery once, instead of every few seconds.
fn track(
    failing: &mut HashSet<String>,
    name: &str,
    error: Option<&crate::providers::ProviderError>,
) {
    match error {
        Some(error) if failing.insert(name.to_owned()) => {
            warn!(provider = name, "read failed: {error}");
        }
        None if failing.remove(name) => info!(provider = name, "read recovered"),
        _ => {}
    }
}

struct RowBuilder {
    start: DateTime<Utc>,
    duration_s: f64,
    cpu: ComponentAccumulator,
    gpu: ComponentAccumulator,
    system: ComponentAccumulator,
    display_known_s: f64,
    display_on_s: f64,
    system_covered_s: f64,
    peak_power_w: Option<f64>,
}

impl RowBuilder {
    fn new(start: DateTime<Utc>) -> Self {
        Self {
            start,
            duration_s: 0.0,
            cpu: ComponentAccumulator::default(),
            gpu: ComponentAccumulator::default(),
            system: ComponentAccumulator::default(),
            display_known_s: 0.0,
            display_on_s: 0.0,
            system_covered_s: 0.0,
            peak_power_w: None,
        }
    }

    fn add(
        &mut self,
        elapsed_s: f64,
        readings: &[(Component, Quality, Reading)],
        display_on: Option<bool>,
    ) {
        self.duration_s += elapsed_s;
        let totals = |component: Component| {
            TickTotals::of(readings.iter().filter(|(c, ..)| *c == component), elapsed_s)
        };
        let (cpu, gpu, system) = (
            totals(Component::Cpu),
            totals(Component::Gpu),
            totals(Component::System),
        );
        if let Some(peak) = [cpu.power_w, gpu.power_w]
            .into_iter()
            .flatten()
            .reduce(|a, b| a + b)
        {
            self.peak_power_w = Some(self.peak_power_w.map_or(peak, |max| max.max(peak)));
        }
        if system.energy_j.is_some() {
            self.system_covered_s += elapsed_s;
        }
        self.cpu.add(&cpu, elapsed_s);
        self.gpu.add(&gpu, elapsed_s);
        self.system.add(&system, elapsed_s);
        if let Some(on) = display_on {
            self.display_known_s += elapsed_s;
            if on {
                self.display_on_s += elapsed_s;
            }
        }
    }

    fn finish(self) -> RawSample {
        // Seconds with an unknown display state take the share observed in the rest of the row.
        let display_on_s = (self.display_known_s > 0.0)
            .then(|| self.display_on_s * self.duration_s / self.display_known_s);
        RawSample {
            start: self.start,
            duration_s: self.duration_s,
            cpu: self.cpu.finish(),
            gpu: self.gpu.finish(),
            system: self.system.finish(),
            display_on_s,
            system_covered_s: self.system_covered_s,
            peak_power_w: self.peak_power_w,
        }
    }
}

/// One component's readings within a single tick, possibly from several devices.
#[derive(Default)]
struct TickTotals {
    energy_j: Option<f64>,
    power_w: Option<f64>,
    util_pct: Option<f64>,
    quality: Option<Quality>,
    failed: bool,
}

impl TickTotals {
    fn of<'a>(
        readings: impl Iterator<Item = &'a (Component, Quality, Reading)>,
        elapsed_s: f64,
    ) -> Self {
        let mut totals = Self::default();
        let mut util = (0.0, 0_u32);
        for (_, quality, reading) in readings {
            totals.failed |= *quality == Quality::Missing;
            if let Some(energy) = reading.energy_j {
                totals.energy_j = Some(totals.energy_j.unwrap_or(0.0) + energy);
                totals.quality = Some(totals.quality.map_or(*quality, |worst| worst.min(*quality)));
            }
            if let Some(power) = reading
                .power_w
                .or_else(|| reading.energy_j.and_then(|j| average_power(j, elapsed_s)))
            {
                totals.power_w = Some(totals.power_w.unwrap_or(0.0) + power);
            }
            if let Some(value) = reading.util_pct {
                util = (util.0 + value, util.1 + 1);
            }
        }
        totals.util_pct = (util.1 > 0).then(|| util.0 / f64::from(util.1));
        totals
    }
}

#[derive(Default)]
struct ComponentAccumulator {
    util_weighted: f64,
    util_seconds: f64,
    util_max: Option<f64>,
    energy_j: Option<f64>,
    power_max_w: Option<f64>,
    quality: Option<Quality>,
    failed: bool,
}

impl ComponentAccumulator {
    fn add(&mut self, tick: &TickTotals, elapsed_s: f64) {
        self.failed |= tick.failed;
        if let (Some(energy), Some(quality)) = (tick.energy_j, tick.quality) {
            self.energy_j = Some(self.energy_j.unwrap_or(0.0) + energy);
            self.quality = Some(self.quality.map_or(quality, |worst| worst.min(quality)));
        }
        if let Some(power) = tick.power_w {
            self.power_max_w = Some(self.power_max_w.map_or(power, |max| max.max(power)));
        }
        if let Some(util) = tick.util_pct {
            self.util_weighted += util * elapsed_s;
            self.util_seconds += elapsed_s;
            self.util_max = Some(self.util_max.map_or(util, |max| max.max(util)));
        }
    }

    fn finish(self) -> ComponentStats {
        ComponentStats {
            util_avg: (self.util_seconds > 0.0).then(|| self.util_weighted / self.util_seconds),
            util_max: self.util_max,
            energy_j: self.energy_j,
            power_max_w: self.power_max_w,
            quality: if self.failed {
                Quality::Missing
            } else {
                self.quality.unwrap_or_default()
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use chrono::{TimeDelta, TimeZone};

    use super::*;
    use crate::config::CollectorConfig;
    use crate::providers::ProviderError;

    struct Steady {
        component: Component,
        quality: Quality,
        watts: f64,
        util: Option<f64>,
        failing: Rc<Cell<bool>>,
    }

    impl Provider for Steady {
        fn name(&self) -> &str {
            "steady"
        }
        fn component(&self) -> Component {
            self.component
        }
        fn quality(&self) -> Quality {
            self.quality
        }
        fn sample(&mut self, elapsed_s: f64) -> Result<Reading, ProviderError> {
            if self.failing.get() {
                return Err(ProviderError::Device("unplugged".to_owned()));
            }
            Ok(Reading {
                energy_j: Some(self.watts * elapsed_s),
                power_w: Some(self.watts),
                util_pct: self.util,
            })
        }
    }

    /// A cumulative energy counter, like RAPL or NVML, that keeps counting between reads.
    struct EnergyCounter {
        total_j: Rc<Cell<f64>>,
        last_j: Option<f64>,
        failing: Rc<Cell<bool>>,
    }

    impl Provider for EnergyCounter {
        fn name(&self) -> &str {
            "counter"
        }
        fn component(&self) -> Component {
            Component::Cpu
        }
        fn quality(&self) -> Quality {
            Quality::Measured
        }
        fn sample(&mut self, _elapsed_s: f64) -> Result<Reading, ProviderError> {
            if self.failing.get() {
                return Err(ProviderError::Device("unplugged".to_owned()));
            }
            let now = self.total_j.get();
            let energy_j = self.last_j.replace(now).map(|last| now - last);
            Ok(Reading {
                energy_j,
                ..Reading::default()
            })
        }
        fn reset(&mut self) {
            self.last_j = None;
        }
    }

    /// A collector over one energy counter; `step` adds energy to the counter, then ticks.
    struct CounterFixture {
        collector: Collector,
        total_j: Rc<Cell<f64>>,
        failing: Rc<Cell<bool>>,
    }

    impl CounterFixture {
        fn new() -> Self {
            let total_j = Rc::new(Cell::new(0.0));
            let failing = Rc::new(Cell::new(false));
            let counter = EnergyCounter {
                total_j: total_j.clone(),
                last_j: None,
                failing: failing.clone(),
            };
            let collector =
                Collector::new(vec![Box::new(counter)], None, &CollectorConfig::default());
            Self {
                collector,
                total_j,
                failing,
            }
        }

        fn step(&mut self, added_j: f64, now: DateTime<Utc>) -> Option<RawSample> {
            self.total_j.set(self.total_j.get() + added_j);
            self.collector.tick(now)
        }
    }

    struct Screen(Rc<Cell<Option<bool>>>);

    impl DisplaySensor for Screen {
        fn name(&self) -> &str {
            "screen"
        }
        fn is_on(&mut self) -> Result<bool, ProviderError> {
            self.0
                .get()
                .ok_or_else(|| ProviderError::Device("unknown".to_owned()))
        }
    }

    struct Fixture {
        collector: Collector,
        failing: Rc<Cell<bool>>,
        screen: Rc<Cell<Option<bool>>>,
    }

    fn fixture() -> Fixture {
        let failing = Rc::new(Cell::new(false));
        let screen = Rc::new(Cell::new(Some(true)));
        let cpu = Steady {
            component: Component::Cpu,
            quality: Quality::Measured,
            watts: 50.0,
            util: Some(40.0),
            failing: failing.clone(),
        };
        let gpu = Steady {
            component: Component::Gpu,
            quality: Quality::Measured,
            watts: 300.0,
            util: Some(90.0),
            failing: Rc::new(Cell::new(false)),
        };
        let collector = Collector::new(
            vec![Box::new(cpu), Box::new(gpu)],
            Some(Box::new(Screen(screen.clone()))),
            &CollectorConfig::default(),
        );
        Fixture {
            collector,
            failing,
            screen,
        }
    }

    fn at(minute: u32, second: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 1, 15, 12, minute, second)
            .unwrap()
    }

    /// Ticks every 2 s from `from` (inclusive) to `to` (inclusive) and returns completed rows.
    fn run(collector: &mut Collector, from: DateTime<Utc>, to: DateTime<Utc>) -> Vec<RawSample> {
        let mut rows = Vec::new();
        let mut now = from;
        while now <= to {
            rows.extend(collector.tick(now));
            now += TimeDelta::seconds(2);
        }
        rows
    }

    #[test]
    fn one_full_minute_becomes_one_row() {
        let mut f = fixture();
        let rows = run(&mut f.collector, at(0, 0), at(1, 2));
        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        assert_eq!(row.start, at(0, 0));
        assert_eq!(row.duration_s, 60.0);
        assert_eq!(row.cpu.energy_j, Some(3000.0));
        assert_eq!(row.gpu.energy_j, Some(18000.0));
        assert_eq!(row.cpu.util_avg, Some(40.0));
        assert_eq!(row.gpu.power_max_w, Some(300.0));
        assert_eq!(row.cpu.quality, Quality::Measured);
        assert_eq!(row.system.quality, Quality::Missing);
        assert_eq!(row.display_on_s, Some(60.0));
    }

    #[test]
    fn gap_closes_row_and_rebaselines() {
        let mut f = fixture();
        run(&mut f.collector, at(0, 0), at(0, 10));
        let closed = f.collector.tick(at(30, 0));
        let row = closed.expect("the open row is closed by the gap");
        assert_eq!(row.duration_s, 10.0);
        assert_eq!(row.cpu.energy_j, Some(500.0));
        let rows = run(&mut f.collector, at(30, 2), at(31, 2));
        assert_eq!(rows[0].start, at(30, 0));
        assert_eq!(rows[0].duration_s, 60.0);
    }

    #[test]
    fn clock_going_backwards_is_a_gap() {
        let mut f = fixture();
        run(&mut f.collector, at(0, 0), at(0, 10));
        assert!(f.collector.tick(at(0, 4)).is_some());
    }

    #[test]
    fn failing_provider_leaves_its_component_missing() {
        let mut f = fixture();
        f.failing.set(true);
        let rows = run(&mut f.collector, at(0, 0), at(1, 2));
        assert_eq!(rows[0].cpu.energy_j, None);
        assert_eq!(rows[0].cpu.quality, Quality::Missing);
        assert_eq!(rows[0].gpu.energy_j, Some(18000.0));
    }

    #[test]
    fn unknown_display_seconds_are_scaled() {
        let mut f = fixture();
        run(&mut f.collector, at(0, 0), at(0, 30));
        f.screen.set(None);
        let rows = run(&mut f.collector, at(0, 32), at(1, 2));
        assert_eq!(rows[0].display_on_s, Some(60.0));
        f.screen.set(Some(false));
        let rows = run(&mut f.collector, at(1, 4), at(2, 2));
        assert_eq!(rows[0].display_on_s, Some(0.0));
    }

    #[test]
    fn flush_returns_the_partial_row() {
        let mut f = fixture();
        run(&mut f.collector, at(0, 0), at(0, 20));
        let row = f.collector.flush().unwrap();
        assert_eq!(row.duration_s, 20.0);
        assert!(f.collector.flush().is_none());
    }

    #[test]
    fn energy_counted_during_an_outage_is_not_attributed() {
        let mut f = CounterFixture::new();
        f.step(0.0, at(0, 0));
        f.step(100.0, at(0, 2));
        f.step(100.0, at(0, 4));
        f.failing.set(true);
        f.step(5000.0, at(0, 6));
        f.failing.set(false);
        f.step(100.0, at(0, 8));
        f.step(100.0, at(0, 10));
        assert_eq!(f.collector.flush().unwrap().cpu.energy_j, Some(300.0));
    }

    #[test]
    fn energy_counted_across_a_gap_is_not_attributed() {
        let mut f = CounterFixture::new();
        f.step(0.0, at(0, 0));
        f.step(100.0, at(0, 2));
        assert!(f.step(9000.0, at(30, 0)).is_some());
        f.step(100.0, at(30, 2));
        assert_eq!(f.collector.flush().unwrap().cpu.energy_j, Some(100.0));
    }

    #[test]
    fn rows_follow_a_shorter_write_interval() {
        let config = CollectorConfig {
            write_interval_s: 30,
            ..CollectorConfig::default()
        };
        let mut f = fixture();
        f.collector = Collector::new(Vec::new(), None, &config);
        let rows = run(&mut f.collector, at(0, 0), at(1, 2));
        let starts: Vec<_> = rows.iter().map(|row| row.start).collect();
        assert_eq!(starts, [at(0, 0), at(0, 30)]);
        assert!(rows.iter().all(|row| row.duration_s == 30.0));
    }

    #[test]
    fn a_failed_read_marks_the_component_missing() {
        let mut f = fixture();
        run(&mut f.collector, at(0, 0), at(0, 30));
        f.failing.set(true);
        f.collector.tick(at(0, 32));
        f.failing.set(false);
        let rows = run(&mut f.collector, at(0, 34), at(1, 2));
        assert_eq!(rows[0].cpu.quality, Quality::Missing);
        assert_eq!(rows[0].gpu.quality, Quality::Measured);
    }

    #[test]
    fn peak_is_the_combined_power_of_one_interval() {
        let mut f = fixture();
        let rows = run(&mut f.collector, at(0, 0), at(1, 2));
        assert_eq!(rows[0].peak_power_w, Some(350.0));
    }

    struct Battery(Rc<Cell<bool>>);

    impl Provider for Battery {
        fn name(&self) -> &str {
            "battery"
        }
        fn component(&self) -> Component {
            Component::System
        }
        fn quality(&self) -> Quality {
            Quality::Reported
        }
        fn sample(&mut self, elapsed_s: f64) -> Result<Reading, ProviderError> {
            Ok(if self.0.get() {
                Reading {
                    energy_j: Some(20.0 * elapsed_s),
                    power_w: Some(20.0),
                    util_pct: None,
                }
            } else {
                Reading::default()
            })
        }
    }

    #[test]
    fn seconds_on_battery_are_recorded() {
        let on_battery = Rc::new(Cell::new(true));
        let mut collector = Collector::new(
            vec![Box::new(Battery(on_battery.clone()))],
            None,
            &CollectorConfig::default(),
        );
        run(&mut collector, at(0, 0), at(0, 30));
        on_battery.set(false);
        let rows = run(&mut collector, at(0, 32), at(1, 2));
        assert_eq!(rows[0].system_covered_s, 30.0);
        assert_eq!(rows[0].system.energy_j, Some(600.0));
    }
}
