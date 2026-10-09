# wattcost v0.1 design

Date: 2026-10-09. Status: draft for review.

## Goal

`wattcost` is a background logger that measures how much electricity a computer uses and what it costs. It records one row per minute with the measured energy of the main components, whether the monitor was on, the energy drawn from the wall and its price under the user's tariff. A report command turns the rows into day, week and month tables split by tariff period (for example day and night).

Success for v0.1: on a Linux desktop with an NVIDIA GPU, `wattcost run` collects data for a day without gaps while the machine is up, and `wattcost report --day` shows kWh and cost per tariff period that match an independent check (component counters and `nvidia-smi`) within the precision of the configured constants.

## Scope

In v0.1:

1. Linux only. The code is organised so that macOS and Windows providers can be added later without touching the collector, storage or report.
2. One Rust binary with the commands `detect`, `run`, `report`, `reprice` and `setup`.
3. Everything that differs between machines and users lives in a config file: hardware constants, tariff periods, prices, fees, taxes, currency, intervals, paths.
4. SQLite storage.

Out of scope for v0.1: macOS and Windows providers, the panel/tray application, smart-plug providers, consumption tiers by monthly total (zones), packaging, releases, publishing.

## Architecture

```
providers ──samples every 2 s──> collector ──one row per minute──> store (SQLite)
                                     │                                 │
config.toml ──hardware + tariff──────┘                                 └──> report / reprice
```

1. **Providers.** One module per sensor, all behind one trait. Each provider reports what it can (energy counter, utilisation, display state) and a quality class:
   1. `measured`: an energy counter (exact energy over an interval);
   2. `reported`: an instantaneous or short-average power reading integrated by the collector;
   3. `estimated`: a model (utilisation times configured wattage).
2. **Collector.** Polls every provider every `sample_interval` (default 2 s), keeps running minimums, maximums and energy deltas in memory, and writes one row at every wall-clock minute boundary (`write_interval`, default 60 s, must divide 60 s). Rows are aligned to UTC multiples of the write interval; because every time-zone offset is a whole number of minutes and tariff periods have minute precision, a row never spans two tariff periods.
3. **Pricing.** When a row is written, the collector converts component energy to wall energy and prices it with the tariff in force at that minute. The price and cost are stored in the row.
4. **Store.** SQLite file, schema migrations embedded in the binary.
5. **Report and reprice.** `report` reads stored rows only. `reprice` recomputes wall energy and cost for a time range from the raw columns and the current config.

## Providers in v0.1

| Provider | Source | Gives | Quality |
|---|---|---|---|
| `rapl` | `/sys/class/powercap/intel-rapl:*` package domains (Intel and AMD Zen) | CPU package energy | measured |
| `cpu_stat` | `/proc/stat` | CPU utilisation | measured |
| `cpu_estimate` | `cpu_stat` plus `hardware.cpu_idle_watts` / `cpu_max_watts` | CPU power when no CPU energy source is available | estimated |
| `nvml` | `libnvidia-ml.so` loaded at runtime (`nvmlDeviceGetTotalEnergyConsumption`, utilisation, power) | GPU energy, utilisation, peak power per NVIDIA device | measured |
| `amdgpu` | hwmon `power1_average` / `power1_input` and `gpu_busy_percent` | AMD GPU power and utilisation | reported |
| `amdgpu` (integrated) | the same hwmon on an AMD APU or Ryzen iGPU (no `mem_info_vram_vendor`) reports the whole CPU package (PPT); used as CPU power when RAPL is not readable, skipped otherwise | CPU package power | reported |
| `battery` | `/sys/class/power_supply/*/power_now` while discharging | whole-system power on a laptop running on battery | reported |
| `display` | GNOME Mutter D-Bus `org.gnome.Mutter.DisplayConfig.PowerSaveMode` (0 = on), connected lazily so a collector started before the desktop session picks it up later; fallback per sample: DRM `/sys/class/drm/*/dpms` of connected and enabled outputs | monitor on or off | measured |

Rules:

1. `detect` probes every provider and prints what was found, the device names, the quality class and why a provider is unavailable (missing file, permission denied, library not found).
2. Several GPUs are summed; the row stores the worst quality among them.
3. A provider that fails at runtime is logged and skipped; the row records the missing value as NULL and its quality as `missing`. The collector never stops because one provider fails.
4. `hardware.providers` in the config can force a provider on or off.
5. The RAPL counter wraps at `max_energy_range_uj`; deltas handle the wrap. NVML energy is in millijoules and monotonic while the driver is loaded; a decrease means a driver reload and that interval is dropped.
6. AMD GPU and battery providers are tested against fake sysfs trees only; no real hardware is available for them in v0.1.
7. The DRM `dpms` fallback is unverified on NVIDIA and may always read `On`; `detect` says so.

### RAPL permissions

Since the PLATYPUS side-channel fix (CVE-2020-8694), `energy_uj` is readable by root only. `setup` offers a udev rule that makes the package `energy_uj` files readable by one group, and prints the exact `sudo` commands instead of running them. The README states the trade-off: any member of that group regains access to the fine-grained energy counter that PLATYPUS used. Without the rule, `wattcost` falls back to `cpu_estimate`.

## Energy and price per row

For a row of `duration_s` seconds:

```
components_j = cpu_energy_j + gpu_energy_j + base_watts * duration_s
wall_wh      = components_j / psu_efficiency / 3600
             + monitor_watts * display_on_s / 3600
```

When the `battery` provider reports whole-system energy (a laptop on battery), it replaces `components_j / psu_efficiency` for the seconds it covers (`system_covered_s`); the rest of the row uses the component formula scaled to the remaining share. `monitor_watts` then means an external monitor only.

`base_watts` covers the motherboard, memory, storage and fans. The monitor has its own power supply, so it is not divided by `psu_efficiency`. If the display state is unknown, `hardware.monitor_assumed_on` (default `true`) decides.

Price per kWh for the tariff period the row falls into:

```
price_per_kwh = (period.price * energy_multiplier + sum(per_kwh_fees)) * product(tax_multipliers)
cost          = wall_wh / 1000 * price_per_kwh
```

`energy_multiplier` holds discounts that apply to energy only; `per_kwh_fees` are fixed surcharges per kWh; `tax_multipliers` are applied in order (for example excise, then VAT). Fixed monthly charges that do not depend on consumption are not part of the per-row cost.

## Prices are stored per row

Each row stores the wall energy, the period name, the effective price per kWh, the cost and the currency that were in force when it was written. Changing the config affects new rows only. History is changed on purpose with `reprice`:

```
wattcost reprice --from 2026-10-01 [--to 2026-10-31] [--dry-run]
```

`reprice` recomputes `wall_wh`, `period`, `price_per_kwh`, `cost`, `currency` and `settings_id` for the range from the raw columns and the current config, in one transaction, and prints the totals before and after. `--dry-run` prints the totals without writing. Raw measurements are never modified.

The settings used for each row are kept: on start and on every config reload the collector stores the full config text in `settings` if it differs from the last stored one, and every row references it through `settings_id`.

## Storage

Default path `~/.local/share/wattcost/wattcost.db`, configurable.

```sql
CREATE TABLE settings (
  id          INTEGER PRIMARY KEY,
  created_at  INTEGER NOT NULL,      -- unix seconds, UTC
  config_toml TEXT    NOT NULL
);

CREATE TABLE samples (
  id              INTEGER PRIMARY KEY,
  ts_start        INTEGER NOT NULL,    -- unix seconds, UTC, aligned to write_interval_s
  duration_s      REAL    NOT NULL,    -- seconds actually covered (< 60 after start or a gap)
  cpu_util_avg    REAL,                -- percent
  cpu_util_max    REAL,
  cpu_energy_j    REAL,
  cpu_power_max_w REAL,
  cpu_quality     INTEGER NOT NULL,    -- 0 missing, 1 estimated, 2 reported, 3 measured
  gpu_util_avg    REAL,
  gpu_util_max    REAL,
  gpu_energy_j    REAL,
  gpu_power_max_w REAL,
  gpu_quality     INTEGER NOT NULL,
  system_energy_j REAL,                -- whole-system energy from the battery provider
  system_quality  INTEGER NOT NULL,
  display_on_s    REAL,                -- NULL when unknown
  wall_wh         REAL    NOT NULL,
  period          TEXT    NOT NULL,
  price_per_kwh   REAL    NOT NULL,
  cost            REAL    NOT NULL,
  currency        TEXT    NOT NULL,
  settings_id     INTEGER NOT NULL REFERENCES settings(id)
);
CREATE INDEX samples_ts ON samples(ts_start);
```

A restart inside a minute can produce two partial rows for the same `ts_start`; both are kept and both count.

About 150 bytes per row, about 6 MB per month at one row per minute. `PRAGMA user_version` holds the schema version.

## Gaps, sleep and restarts

1. A row covers only the seconds that were actually sampled; `duration_s` is shorter after start-up and around gaps.
2. A pause between two samples longer than `3 * sample_interval` (suspend, a stopped service) is a gap: no energy is attributed to it, and the counters are re-baselined.
3. The partial minute is flushed on SIGTERM and SIGINT.
4. SIGHUP or a change of the config file's mtime reloads the config; an invalid config is logged and the previous one stays in force. A reload applies the hardware constants and the tariff; collector intervals, the database path and provider selection apply after a restart, and the log says so.
5. Times are stored in UTC; tariff periods use the system's local time zone, so daylight-saving changes follow the local clock.

## Config

Path `~/.config/wattcost/config.toml`. The repository ships `config.example.toml` with neutral sample values; no user's real prices go into the repository.

```toml
[collector]
sample_interval_s = 2
write_interval_s  = 60
database          = "~/.local/share/wattcost/wattcost.db"

[hardware]
base_watts         = 40.0
psu_efficiency     = 0.90
monitor_watts      = 40.0
monitor_assumed_on = true
cpu_idle_watts     = 20.0   # used only by cpu_estimate
cpu_max_watts      = 120.0  # used only by cpu_estimate
# providers = { amdgpu = false }

[tariff]
currency          = "EUR"
energy_multiplier = 1.0
per_kwh_fees      = [0.01]
tax_multipliers   = [1.20]

[[tariff.periods]]
name  = "day"
start = "07:00"
end   = "23:00"
price = 0.20

[[tariff.periods]]
name  = "night"
start = "23:00"
end   = "07:00"
price = 0.10
```

Validation: periods must cover all 24 hours without overlap (a period may cross midnight; `start == end` means the whole day), times have minute precision, multipliers and prices are non-negative, `psu_efficiency` is in (0, 1], `write_interval_s` divides 60 and is a multiple of `sample_interval_s`. `wattcost` without a config file uses the example values and says so in `detect` and in reports.

## Commands

1. `wattcost detect`: providers, devices, quality, permission hints, the config in use.
2. `wattcost run`: the collector, in the foreground; logs to stderr (journald under systemd).
3. `wattcost report [--day|--week|--month] [--from DATE --to DATE]`: per tariff period and in total: hours covered, hours with the monitor on, average and peak power, kWh, cost; plus the share of rows by data quality. The current partial minute is not included.
4. `wattcost reprice --from DATE [--to DATE] [--dry-run]`: see above.
5. `wattcost setup`: writes the systemd user unit `wattcost.service` (enabled, `Restart=on-failure`) and prints the udev rule and the `sudo` commands for RAPL access. It does not run `sudo` itself.

## Code layout

```
src/main.rs            thin entry point: parse arguments, call into the library
src/lib.rs             CLI dispatch (clap) and module declarations
src/config.rs          config parsing and validation
src/tariff.rs          period lookup and price formula
src/energy.rs          wall-energy formula
src/collector.rs       sampling loop, minute aggregation, gaps
src/store.rs           SQLite schema, migrations, queries
src/report.rs          report tables
src/reprice.rs         range repricing
src/providers/mod.rs   Provider trait, registry, detection
src/providers/{rapl,cpu_stat,cpu_estimate,nvml,amdgpu,battery,display}.rs
```

Crates: `clap`, `serde`, `toml`, `rusqlite` (bundled SQLite), `nvml-wrapper` (loads the library at runtime, so the binary runs without NVIDIA drivers), `zbus` (blocking API), `chrono` with the local time zone, `thiserror`, `anyhow`, `tracing`, `signal-hook`.

## Code conventions

1. Rust 2024 edition, stable toolchain pinned in `rust-toolchain.toml` with `clippy` and `rustfmt`; `rust-version` declared in `Cargo.toml`.
2. Code follows the `rust-skills` guidelines (leonardomso/rust-skills, the same skill the agentic-playbooks repo uses), vendored into the repo as a project skill.
3. Lints in `Cargo.toml` `[lints]`: `clippy::correctness` deny; `suspicious`, `style`, `complexity`, `perf` warn; selected `pedantic` lints; `unsafe_code = "forbid"` (NVML and D-Bus are reached through safe crates).
4. Gates: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`.
5. Library errors use typed error enums (`thiserror`); `anyhow` only in the CLI layer.

## Open-source readiness

v0.1 is not published, but the repository is ready to be:

1. Apache License 2.0 (`LICENSE`, `NOTICE`), declared in `Cargo.toml` together with the repository URL, description, keywords and categories.
2. `README.md`: what it measures and what it cannot (wall power is an estimate unless measured externally), supported hardware, install, RAPL permission trade-off, config reference, report example.
3. `CONTRIBUTING.md` (gates, how to add a provider), `SECURITY.md`, `CHANGELOG.md`, `.gitignore`, `config.example.toml`.
4. A GitHub Actions workflow running the gates on Linux; it only takes effect once the repository is pushed.
5. Code: SOLID and DRY, small focused modules, English identifiers and docs, comments only where the reason is not obvious from the code.
6. Nothing in the repository names the author's machine, location, utility or real prices.

## Testing

1. Tariff: period lookup including periods that cross midnight and minutes on period edges; the price formula with discounts, fees and chained taxes; config validation errors.
2. Energy: the wall formula, unknown display state, PSU efficiency.
3. Collector: minute alignment, partial minutes, gap detection, RAPL wrap, NVML counter reset, max and average aggregation; driven by fake providers and a fake clock.
4. Store and reprice: migrations on an empty database, repricing a range with a changed tariff, dry run leaves data untouched, raw columns unchanged.
5. Providers: `rapl`, `cpu_stat`, `amdgpu`, `battery` and the DRM display fallback against fake sysfs and procfs trees.
6. Manual acceptance on the development machine (an AMD desktop CPU with an integrated GPU, an NVIDIA GPU, GNOME on Wayland): `detect` finds RAPL, NVML and the Mutter display state; one day of `run` under systemd; `report --day` compared with the RAPL and NVML counters read directly and with `nvidia-smi` power readings; monitor on and off times match the screen blanking.

## Later (not v0.1)

1. macOS (IOReport, SMC `PSTR`, `CGDisplayIsAsleep`) and Windows (NVML, display power notifications, CPU estimate) providers.
2. A panel application that shows live figures and edits the config.
3. Smart-plug providers (Shelly, Tasmota, TP-Link) for real wall power.
4. Consumption tiers by monthly total.
