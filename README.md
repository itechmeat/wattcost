# wattcost

[![CI](https://github.com/itechmeat/wattcost/actions/workflows/ci.yml/badge.svg)](https://github.com/itechmeat/wattcost/actions/workflows/ci.yml)

wattcost shows what your Linux computer costs you in electricity. It measures CPU, GPU and monitor power in the background, prices every minute with your day and night tariff, and puts today's cost in the GNOME top bar with a chart for the day, week and month.

![The top bar indicator: a week of spending and power, and the settings card](https://raw.githubusercontent.com/itechmeat/wattcost/main/docs/screenshot.png)

## Install

### The easy way: ask your coding agent

Paste this into Claude Code, Codex or another coding agent on the computer:

```
Install wattcost for me by following https://raw.githubusercontent.com/itechmeat/wattcost/main/llms.txt
```

The agent checks your system, installs wattcost, asks for your electricity prices (you can send it a photo of your bill) and sets up the service and the top bar indicator. [llms.txt](llms.txt) holds its instructions.

### The manual way

1. Install the binary in one of three ways:
   ```sh
   # A release build, no Rust needed (Linux x86_64 or aarch64):
   curl -fsSL https://raw.githubusercontent.com/itechmeat/wattcost/main/install.sh | sh
   # The same release build through cargo-binstall:
   cargo binstall wattcost
   # Or build it from crates.io (needs Rust and a C compiler):
   cargo install wattcost --locked
   ```
2. Check the sensors: `wattcost detect`.
3. Set your currency and the final price per kWh (discounts, fees and taxes included) of your day and night rates:
   ```sh
   echo '{"currency":"EUR","day":{"start_hour":7,"end_hour":23,"price":0.25},"night":{"start_hour":23,"end_hour":7,"price":0.13}}' | wattcost config set
   ```
4. Run `wattcost setup`. It starts the background service and, on GNOME 50, installs the top bar indicator; log out and back in once to see the indicator. User services run while you are logged in; `loginctl enable-linger` keeps the service running from boot. Reading the RAPL CPU energy counter needs a udev rule; `setup` prints the commands and explains the security trade-off (CVE-2020-8694). Without it, wattcost uses an AMD integrated GPU's package power when there is one, and otherwise estimates CPU power.

To update, install the new version the same way, then run `systemctl --user restart wattcost` and `wattcost extension install` (the indicator reloads without logging out).

## What it measures

| Part | Source | Quality |
|---|---|---|
| CPU | RAPL energy counter (Intel, AMD Zen) | measured |
| CPU without RAPL access | utilisation × configured idle/max power | estimated |
| NVIDIA GPU | NVML energy counter (Volta and newer) | measured |
| AMD GPU | amdgpu hwmon power | reported |
| CPU on AMD with an integrated GPU, without RAPL access | the iGPU's package power (PPT) | reported |
| Laptop on battery | battery discharge power (whole system) | reported |
| Monitor on/off | GNOME Mutter, DRM `dpms` fallback | measured |

Motherboard, memory, storage, fans, power-supply losses and the monitor's power are configured constants. Wall power is therefore an estimate; only a meter at the socket measures it exactly.

Linux only for now. macOS and Windows support is planned.

## Configure

The gear button in the indicator's menu changes the currency, the hours and prices of the day and night rates, and two constants: the power of the parts without a sensor (motherboard, memory, disks, network and fans) and of the monitor while it is on. For the best numbers, take them from a wall power meter or the monitor's datasheet. `wattcost config show` and `wattcost config set` do the same from a terminal. Everything else lives in `~/.config/wattcost/config.toml` (see [config.example.toml](config.example.toml)). The running service picks up changes automatically; collector intervals and provider switches apply after `systemctl --user restart wattcost`.

The file can also describe a bill the way it is printed, with more periods and separate discounts, fees and taxes. The price of one kWh in a period is then

```
(price × energy_multiplier + sum(per_kwh_fees)) × product(tax_multipliers)
```

The settings form handles two periods with final prices; for anything else edit the file.

Prices are stored with every row. After a price change that applies retroactively, recompute a range:

```sh
wattcost reprice --from 2026-10-01 --dry-run
wattcost reprice --from 2026-10-01
```

## Report

```sh
wattcost report            # today
wattcost report --week
wattcost report --month
wattcost report --from 2026-10-01 --to 2026-10-07
```

```
2026-10-09 (local time)
Period         Hours  Monitor h   Avg W  Peak CPU+GPU W       kWh           Cost
day            16.00      12.30     160             410     2.560       0.65 EUR
night           8.00       0.00     120             380     0.960       0.13 EUR
total          24.00      12.30     147             410     3.520       0.77 EUR
CPU: measured 100.0%
GPU: measured 100.0%
```

## Prometheus metrics

`wattcost run` can serve its data in the Prometheus text format, so Prometheus, VictoriaMetrics, Grafana Agent or any other scraper can collect it. The endpoint is off by default. Turn it on in `~/.config/wattcost/config.toml` and restart the service:

```toml
[metrics]
listen = "127.0.0.1:9877"
```

`wattcost run --metrics-listen 127.0.0.1:9877` does the same for one run and overrides the file. The address is an IP address and a port; use `0.0.0.0:9877` (or `[::]:9877`) to let other machines scrape it. Only `GET /metrics` is served.

| Metric | Type | Meaning |
|---|---|---|
| `wattcost_power_watts{component="cpu\|gpu\|base\|monitor"}` | gauge | Average power over the last stored row (one minute by default); `cpu`, `gpu` and `base` before power-supply losses, `base` and `monitor` from the configured constants |
| `wattcost_wall_power_watts` | gauge | Average power drawn from the wall over the last stored row |
| `wattcost_energy_joules_total{component="cpu\|gpu"}` | counter | CPU and GPU energy over all stored rows |
| `wattcost_wall_energy_joules_total` | counter | Wall energy over all stored rows |
| `wattcost_cost_total{currency}` | counter | Cost over all stored rows, at the prices stored with them |
| `wattcost_price_per_kwh{currency,period}` | gauge | Price of one kWh in the tariff period in force now |
| `wattcost_last_sample_timestamp_seconds` | gauge | Start of the interval of the last stored row as a Unix time |
| `wattcost_build_info{version}` | gauge | Always 1 |

Energy is in joules, the Prometheus base unit; divide by 3.6e6 for kWh. The counters start from the totals in the database, so a restart continues them. `wattcost reprice` changes the stored costs; the running service shows the new totals after a restart, which Prometheus sees as a counter reset when the total went down.

A scrape job:

```yaml
scrape_configs:
  - job_name: wattcost
    scrape_interval: 60s
    static_configs:
      - targets: ["desktop.example:9877"]
```

PromQL examples:

```promql
# Wall power now, in watts
wattcost_wall_power_watts
# Energy over the last 24 hours, in kWh
increase(wattcost_wall_energy_joules_total[24h]) / 3.6e6
# Cost over the last 24 hours and the last 30 days
increase(wattcost_cost_total[24h])
increase(wattcost_cost_total[30d])
# Today or this month in Grafana: a "Today so far" or "This month so far" time range and
increase(wattcost_cost_total[$__range])
# Alert when the collector has stopped writing rows
time() - wattcost_last_sample_timestamp_seconds > 300
```

The endpoint has no authentication or TLS and shows when the computer is in use. Keep it on `127.0.0.1`, or bind it to a private network (a VPN or tailnet) and limit the port with a firewall.

## Top bar indicator (GNOME)

The binary carries a GNOME Shell 50 extension (sources in `gnome-extension/`) that shows today's cost in the top bar. Clicking it opens a chart with the spending rate (green), the total power (purple) and the CPU (blue) and GPU (orange) power for today, the last week or the current month, and a gear button for the currency and the day and night rates. It reads the data with `wattcost series --span day|week|month`, which prints JSON.

`wattcost setup` installs it; `wattcost extension install` installs or updates it on its own. On Wayland, GNOME loads a newly installed extension after you log out and back in; later updates reload it in the running session.

## Other status bars (Waybar, Omarchy)

`wattcost status` prints today's cost and the current power on one line, for example `1.23 EUR · 45 W`; the power is left out when the last row started more than three write intervals ago. `wattcost status --format waybar` prints the same as Waybar custom-module JSON, with today's energy, average and peak power in the tooltip and the class `ok` or `stale`; when the config or the database cannot be read it prints `wattcost: error` with the reason in the tooltip and the class `error`, so the module does not vanish.

A Waybar module (`~/.config/waybar/config.jsonc`, add `"custom/wattcost"` to a `modules-*` list):

```jsonc
"custom/wattcost": {
  "exec": "wattcost status --format waybar",
  "return-type": "json",
  "interval": 60
}
```

The Omarchy shell bar (`~/.config/omarchy/shell.json`, inside `bar.layout.left`, `center` or `right`) reads the same JSON:

```json
{ "id": "wattcost", "type": "command", "exec": "wattcost status --format waybar", "interval": 60 }
```

If the bar cannot find the binary, give its full path, for example `~/.local/bin/wattcost`.

## License

Licensed under the [Apache License, Version 2.0](LICENSE).
