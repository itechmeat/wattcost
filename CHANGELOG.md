# Changelog

All notable changes to this project are documented here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses [Semantic Versioning](https://semver.org/).

## [Unreleased]

## [0.1.1] - 2026-10-09

### Added

- The settings card and `config show`/`config set` also edit the system power (motherboard, memory, disks, network and fans) and the monitor power, with a short hint for each.

### Changed

- `config set` requires `base_watts` and `monitor_watts` in its JSON.

### Fixed

- The extension's unit tests compared nothing, so they could not fail.

## [0.1.0] - 2026-10-09

### Added

- Logger for Linux: CPU energy (RAPL, an AMD integrated GPU's package power, or an estimate from utilisation), NVIDIA GPUs (NVML), AMD GPUs (hwmon), laptop battery power and the monitor's on/off state (GNOME Mutter, DRM fallback), recorded once a minute in SQLite.
- Time-of-use pricing with periods, discounts, per-kWh fees and taxes; prices are stored with every row and `reprice` recomputes a date range.
- Commands: `detect`, `run`, `report`, `reprice`, `setup` (systemd user service and the optional RAPL udev rule), `series` (JSON for charts) and `config show`/`config set` (currency and day and night rates).
- GNOME Shell 50 extension: today's cost in the top bar; a chart of the spending rate, total power and CPU and GPU power for today, the last week or the current month; a settings card for the currency and the day and night rates; `install.sh` reloads it without logging out.
- The extension is built into the binary: `wattcost setup` installs it on GNOME and `wattcost extension install` installs or updates it.
- Release builds for Linux x86_64 and aarch64 on GitHub, installable with `install.sh`, `cargo binstall wattcost` or `cargo install wattcost`.
- `llms.txt` with installation instructions for coding agents.
