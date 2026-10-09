CREATE TABLE settings (
  id          INTEGER PRIMARY KEY,
  created_at  INTEGER NOT NULL,
  config_toml TEXT    NOT NULL
);

CREATE TABLE samples (
  id              INTEGER PRIMARY KEY,
  ts_start        INTEGER NOT NULL,
  duration_s      REAL    NOT NULL,
  cpu_util_avg    REAL,
  cpu_util_max    REAL,
  cpu_energy_j    REAL,
  cpu_power_max_w REAL,
  cpu_quality     INTEGER NOT NULL,
  gpu_util_avg    REAL,
  gpu_util_max    REAL,
  gpu_energy_j    REAL,
  gpu_power_max_w REAL,
  gpu_quality     INTEGER NOT NULL,
  system_energy_j REAL,
  system_quality  INTEGER NOT NULL,
  display_on_s    REAL,
  system_covered_s REAL   NOT NULL,
  peak_power_w    REAL,
  wall_wh         REAL    NOT NULL,
  period          TEXT    NOT NULL,
  price_per_kwh   REAL    NOT NULL,
  cost            REAL    NOT NULL,
  currency        TEXT    NOT NULL,
  settings_id     INTEGER NOT NULL REFERENCES settings(id)
);

CREATE INDEX samples_ts ON samples(ts_start);
