//! SQLite storage of rows and of the settings they were priced with.

use std::fs;
use std::io;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, Row, params};
use thiserror::Error;

use crate::pricing::Pricing;
use crate::providers::Quality;
use crate::sample::{ComponentStats, RawSample};

const MIGRATIONS: [&str; 1] = [include_str!("../migrations/001_initial.sql")];

const COLUMNS: &str = "id, ts_start, duration_s, \
    cpu_util_avg, cpu_util_max, cpu_energy_j, cpu_power_max_w, cpu_quality, \
    gpu_util_avg, gpu_util_max, gpu_energy_j, gpu_power_max_w, gpu_quality, \
    system_energy_j, system_quality, display_on_s, system_covered_s, peak_power_w, \
    wall_wh, period, price_per_kwh, cost, currency";

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("cannot create {path}: {error}")]
    CreateDir { path: PathBuf, error: io::Error },
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error("database schema version {0} is newer than this version of wattcost supports")]
    TooNew(i64),
    #[error("timestamp {0} is out of range")]
    Timestamp(i64),
}

#[derive(Debug, Clone, PartialEq)]
pub struct StoredSample {
    pub id: i64,
    pub sample: RawSample,
    pub pricing: Pricing,
}

#[derive(Debug)]
pub struct Store {
    conn: Connection,
}

impl Store {
    /// Opens or creates the database; a new directory and file are readable by the owner only,
    /// because the power history shows when the owner is at the computer.
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        let create_error = |path: &Path| {
            let path = path.to_path_buf();
            move |error| StoreError::CreateDir { path, error }
        };
        if let Some(parent) = path.parent() {
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(parent)
                .map_err(create_error(parent))?;
        }
        fs::OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(path)
            .map_err(create_error(path))?;
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.busy_timeout(Duration::from_secs(5))?;
        Self::migrate(conn)
    }

    fn migrate(mut conn: Connection) -> Result<Self, StoreError> {
        let version: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
        let latest = i64::try_from(MIGRATIONS.len()).expect("migration count fits i64");
        if version > latest {
            return Err(StoreError::TooNew(version));
        }
        let tx = conn.transaction()?;
        let mut applied = version;
        for migration in MIGRATIONS
            .iter()
            .skip(usize::try_from(version).unwrap_or(0))
        {
            tx.execute_batch(migration)?;
            applied += 1;
            tx.pragma_update(None, "user_version", applied)?;
        }
        tx.commit()?;
        Ok(Self { conn })
    }

    /// Id of the stored settings with this text, inserting them when they differ from the latest.
    pub fn settings_id(&self, text: &str, now: DateTime<Utc>) -> Result<i64, StoreError> {
        let latest: Option<(i64, String)> = self
            .conn
            .query_row(
                "SELECT id, config_toml FROM settings ORDER BY id DESC LIMIT 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        if let Some((id, stored)) = latest {
            if stored == text {
                return Ok(id);
            }
        }
        self.conn.execute(
            "INSERT INTO settings (created_at, config_toml) VALUES (?1, ?2)",
            params![now.timestamp(), text],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn insert_sample(
        &self,
        sample: &RawSample,
        pricing: &Pricing,
        settings_id: i64,
    ) -> Result<(), StoreError> {
        let (cpu, gpu, system) = (&sample.cpu, &sample.gpu, &sample.system);
        self.conn.execute(
            "INSERT INTO samples (ts_start, duration_s, \
                cpu_util_avg, cpu_util_max, cpu_energy_j, cpu_power_max_w, cpu_quality, \
                gpu_util_avg, gpu_util_max, gpu_energy_j, gpu_power_max_w, gpu_quality, \
                system_energy_j, system_quality, display_on_s, system_covered_s, peak_power_w, \
                wall_wh, period, price_per_kwh, cost, currency, settings_id) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23)",
            params![
                sample.start.timestamp(),
                sample.duration_s,
                cpu.util_avg,
                cpu.util_max,
                cpu.energy_j,
                cpu.power_max_w,
                cpu.quality.code(),
                gpu.util_avg,
                gpu.util_max,
                gpu.energy_j,
                gpu.power_max_w,
                gpu.quality.code(),
                system.energy_j,
                system.quality.code(),
                sample.display_on_s,
                sample.system_covered_s,
                sample.peak_power_w,
                pricing.wall_wh,
                pricing.period,
                pricing.price_per_kwh,
                pricing.cost,
                pricing.currency,
                settings_id,
            ],
        )?;
        Ok(())
    }

    pub fn samples(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> Result<Vec<StoredSample>, StoreError> {
        let mut statement = self
            .conn
            .prepare(&format!("SELECT {COLUMNS} FROM samples WHERE ts_start >= ?1 AND ts_start < ?2 ORDER BY ts_start, id"))?;
        let rows = statement.query_map(params![from.timestamp(), to.timestamp()], read_row)?;
        rows.map(|row| row?.into_stored()).collect()
    }

    pub fn update_pricing(
        &mut self,
        rows: &[(i64, Pricing)],
        settings_id: i64,
    ) -> Result<(), StoreError> {
        let tx = self.conn.transaction()?;
        {
            let mut statement = tx.prepare(
                "UPDATE samples SET wall_wh = ?2, period = ?3, price_per_kwh = ?4, cost = ?5, currency = ?6, settings_id = ?7 \
                 WHERE id = ?1",
            )?;
            for (id, pricing) in rows {
                statement.execute(params![
                    id,
                    pricing.wall_wh,
                    pricing.period,
                    pricing.price_per_kwh,
                    pricing.cost,
                    pricing.currency,
                    settings_id
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }
}

/// A row as read from SQLite, before the timestamp is validated.
struct DbRow {
    id: i64,
    ts_start: i64,
    sample: RawSample,
    pricing: Pricing,
}

impl DbRow {
    fn into_stored(self) -> Result<StoredSample, StoreError> {
        let start = DateTime::from_timestamp(self.ts_start, 0)
            .ok_or(StoreError::Timestamp(self.ts_start))?;
        Ok(StoredSample {
            id: self.id,
            sample: RawSample {
                start,
                ..self.sample
            },
            pricing: self.pricing,
        })
    }
}

fn read_row(row: &Row<'_>) -> rusqlite::Result<DbRow> {
    let component = |offset: usize| -> rusqlite::Result<ComponentStats> {
        Ok(ComponentStats {
            util_avg: row.get(offset)?,
            util_max: row.get(offset + 1)?,
            energy_j: row.get(offset + 2)?,
            power_max_w: row.get(offset + 3)?,
            quality: Quality::from_code(row.get(offset + 4)?),
        })
    };
    Ok(DbRow {
        id: row.get(0)?,
        ts_start: row.get(1)?,
        sample: RawSample {
            start: DateTime::UNIX_EPOCH,
            duration_s: row.get(2)?,
            cpu: component(3)?,
            gpu: component(8)?,
            system: ComponentStats {
                energy_j: row.get(13)?,
                quality: Quality::from_code(row.get(14)?),
                ..ComponentStats::default()
            },
            display_on_s: row.get(15)?,
            system_covered_s: row.get(16)?,
            peak_power_w: row.get(17)?,
        },
        pricing: Pricing {
            wall_wh: row.get(18)?,
            period: row.get(19)?,
            price_per_kwh: row.get(20)?,
            cost: row.get(21)?,
            currency: row.get(22)?,
        },
    })
}

#[cfg(test)]
mod tests {
    use chrono::{TimeDelta, TimeZone};

    use super::*;
    use crate::providers::Quality;
    use crate::sample::ComponentStats;
    use crate::test_support::TempStore;

    fn sample(minute: u32, duration_s: f64) -> RawSample {
        RawSample {
            start: Utc.with_ymd_and_hms(2026, 1, 15, 12, minute, 0).unwrap(),
            duration_s,
            cpu: ComponentStats {
                util_avg: Some(12.5),
                util_max: Some(80.0),
                energy_j: Some(1500.0),
                power_max_w: Some(60.0),
                quality: Quality::Measured,
            },
            gpu: ComponentStats {
                energy_j: Some(600.0),
                quality: Quality::Reported,
                ..ComponentStats::default()
            },
            system: ComponentStats::default(),
            display_on_s: None,
            system_covered_s: 12.5,
            peak_power_w: Some(310.0),
        }
    }

    fn pricing(cost: f64) -> Pricing {
        Pricing {
            wall_wh: 2.0,
            period: "day".to_owned(),
            price_per_kwh: 0.25,
            cost,
            currency: "EUR".to_owned(),
        }
    }

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 1, 15, 12, 0, 0).unwrap()
    }

    #[test]
    fn round_trips_a_sample() {
        let temp = TempStore::new();
        let store = temp.open();
        let id = store.settings_id("a = 1", now()).unwrap();
        store
            .insert_sample(&sample(0, 60.0), &pricing(0.5), id)
            .unwrap();
        let rows = store.samples(now(), now() + TimeDelta::hours(1)).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].sample, sample(0, 60.0));
        assert_eq!(rows[0].pricing, pricing(0.5));
    }

    #[test]
    fn two_rows_for_the_same_minute_are_kept() {
        let temp = TempStore::new();
        let store = temp.open();
        let id = store.settings_id("a = 1", now()).unwrap();
        store
            .insert_sample(&sample(0, 20.0), &pricing(0.1), id)
            .unwrap();
        store
            .insert_sample(&sample(0, 38.0), &pricing(0.2), id)
            .unwrap();
        assert_eq!(
            store
                .samples(now(), now() + TimeDelta::minutes(1))
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn range_is_half_open() {
        let temp = TempStore::new();
        let store = temp.open();
        let id = store.settings_id("a = 1", now()).unwrap();
        for minute in 0..3 {
            store
                .insert_sample(&sample(minute, 60.0), &pricing(0.1), id)
                .unwrap();
        }
        let rows = store
            .samples(now() + TimeDelta::minutes(1), now() + TimeDelta::minutes(2))
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].sample.start, now() + TimeDelta::minutes(1));
    }

    #[test]
    fn settings_are_stored_once_per_distinct_text() {
        let temp = TempStore::new();
        let store = temp.open();
        let first = store.settings_id("a = 1", now()).unwrap();
        assert_eq!(store.settings_id("a = 1", now()).unwrap(), first);
        assert_ne!(store.settings_id("a = 2", now()).unwrap(), first);
    }

    #[test]
    fn update_pricing_changes_only_pricing() {
        let temp = TempStore::new();
        let mut store = temp.open();
        let id = store.settings_id("a = 1", now()).unwrap();
        store
            .insert_sample(&sample(0, 60.0), &pricing(0.5), id)
            .unwrap();
        let row_id = store.samples(now(), now() + TimeDelta::hours(1)).unwrap()[0].id;
        let new_id = store.settings_id("a = 2", now()).unwrap();
        store
            .update_pricing(&[(row_id, pricing(0.9))], new_id)
            .unwrap();
        let row = &store.samples(now(), now() + TimeDelta::hours(1)).unwrap()[0];
        assert_eq!(row.pricing.cost, 0.9);
        assert_eq!(row.sample, sample(0, 60.0));
    }

    #[test]
    fn reopening_keeps_data_and_schema() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/w.db");
        {
            let store = Store::open(&path).unwrap();
            let id = store.settings_id("a = 1", now()).unwrap();
            store
                .insert_sample(&sample(0, 60.0), &pricing(0.5), id)
                .unwrap();
        }
        let store = Store::open(&path).unwrap();
        assert_eq!(
            store
                .samples(now(), now() + TimeDelta::hours(1))
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn the_database_is_private_to_its_owner() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data/wattcost/w.db");
        Store::open(&path).unwrap();
        let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(path.parent().unwrap()), 0o700);
        assert_eq!(mode(&path), 0o600);
    }

    #[test]
    fn a_newer_schema_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("w.db");
        Connection::open(&path)
            .unwrap()
            .pragma_update(None, "user_version", 99)
            .unwrap();
        assert!(matches!(Store::open(&path), Err(StoreError::TooNew(99))));
    }
}
