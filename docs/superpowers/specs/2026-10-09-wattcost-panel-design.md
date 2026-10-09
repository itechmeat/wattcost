# wattcost panel indicator design

Date: 2026-10-09. Status: approved by the owner in chat.

## Goal

A GNOME Shell extension that shows today's electricity cost of the computer in the top bar and, on click, a menu with one line chart (spending rate in green, power in purple) for today, the last week or the current month. As simple as possible: system widgets, system styles and theme colours. A settings card edits the currency and the day and night rates.

## Scope

1. GNOME Shell 50 extension (GJS, ES modules), UUID `wattcost@itechmeat.github.io`, in `gnome-extension/` of the wattcost repository.
2. A new `wattcost series` command that gives the extension everything it needs as JSON.
3. `wattcost config show` and `wattcost config set` for the settings card.
4. Out of scope: `prefs.js`, notifications, other desktops, retroactive repricing from the card.

## Data: `wattcost series --span day|week|month`

Prints one JSON object built from stored rows:

```json
{
  "span": "day",
  "label": "2026-10-09",
  "currency": "EUR",
  "today_cost": 2.41,
  "total_cost": 2.41,
  "total_kwh": 2.2,
  "average_w": 152.0,
  "peak_w": 410.0,
  "cpu_average_w": 47.0,
  "gpu_average_w": 25.0,
  "start_label": "00:00",
  "end_label": "24:00",
  "points": [{ "cost_per_hour": 0.05, "average_w": 120.5, "cpu_w": 46.8, "gpu_w": 25.1 }]
}
```

1. Ranges are those of `report`: today, the last 7 days including today, the current month, in local time.
2. `points` share one time grid: 5-minute intervals for `day`, hourly intervals for `week` and `month`. Each point holds rates, so a partly recorded interval is not drawn low: `cost_per_hour` (cost divided by covered time) `average_w` (wall energy divided by covered time), `cpu_w` and `gpu_w` (component energy divided by the seconds in which that component was recorded); `null` where nothing was recorded.
3. `start_label`/`end_label` name both ends of the time axis: `00:00`–`24:00`, weekday abbreviations (`Sat`–`Fri`), month dates (`Oct 1`–`Oct 9`).
4. `currency` is that of the latest row, or the config's when the range has no rows; rows in an older currency (the tariff changed within the span) are left out of every sum. `today_cost` is always today's total, whatever the span, computed from the span's rows.
5. `average_w` is the wall average over covered time; `peak_w` is the highest combined CPU and GPU power of one sampling interval converted to wall power with the same constants (base load, PSU efficiency, monitor), so the two are comparable; both `null` without data.
6. Pricing and aggregation stay in Rust; the extension only draws.

## Config commands

1. `wattcost config show` prints `{"currency": "EUR", "day": {"start_hour": 7, "end_hour": 23, "price": 0.31}, "night": {...}}`. Prices are final prices per kWh, computed with the tariff's discount, fee and tax fields. It fails with an explanation when the tariff is not exactly a `day` and a `night` period on whole hours; such tariffs are edited in the file.
2. `wattcost config set` reads the same JSON from stdin, checks it (currency 1–8 characters, hours 0–23, non-negative prices, then the full config validation, so overlaps and gaps are reported as "periods "day" and "night" overlap at 22:00" or "no period covers 22:00"), writes the two periods, removes the discount, fee and tax fields (the prices are final) and keeps every other setting and comment (`toml_edit`). The file is replaced atomically through a temporary file in the same directory (a symlinked config is written through the link, the file keeps its permissions, the data is synced before the rename); an inline `tariff` table is converted to a normal table; a missing file starts from the bundled example. The running collector reloads it on the next tick.

## Extension

1. A `PanelMenu.Button` with a symbolic lightning-bolt icon (shipped SVG, coloured by the theme) and a label with today's cost and currency, for example `2.41 EUR`.
2. The menu, top to bottom:
   1. one row with Today, Week and Month: buttons in the `popup-menu-item` style with the menu's own radio-dot icons (`ornament-dot-checked-symbolic` / `ornament-dot-unchecked-symbolic`, `popup-menu-ornament`), 8 px apart; choosing one updates the chart and keeps the menu open; then a separator;
   2. one `St.DrawingArea` with a faint frame in the theme's text colour (horizontal lines at quarters of the height, vertical lines every 6 hours for Today and every day for Week and Month, so an almost empty chart still reads as a chart) and four Cairo lines on the same time axis: the spending rate in GNOME green (`#33d17a`), scaled to its own maximum up to 60 % of the height (with a flat tariff it has the shape of the total power and would otherwise hide under it); total power in GNOME purple (`#c061cb`), CPU in blue (`#3584e4`) and GPU in orange (`#ff7800`), all three on one shared watt scale up to 90 % of the height, so the parts read against the whole; gaps where nothing was recorded, a dot for an isolated point; no fills;
   3. the axis end labels under the chart;
   4. the legend under them: `● Spent: 2.41 EUR · 2.20 kWh` with a green bullet and `● Power: average 152 W · peak 410 W` with a purple bullet, then `● CPU 47 W  ● GPU 25 W` (span averages).
3. Refresh: on enable, every 60 s, and when the menu opens or the span changes. The command runs asynchronously through `Gio.Subprocess`; a running refresh is cancelled on disable.
4. The binary is looked up in `PATH`, `~/.cargo/bin` and `~/.local/bin`. If it is missing or fails, the label shows `–` and the menu shows the error text.
5. Settings, in the style of GNOME's quick settings: a round `icon-button` with `emblem-system-symbolic` at the right of the span row opens a card (`quick-toggle-menu` with its `header`: accent-coloured icon and the title "Settings") that slides open inside the menu like a quick settings menu (height, then the content fades in, 125 ms each) while the chart below fades to low opacity. The card holds: Currency (short entry); a row each for Day and Night with two hour entries, each followed by ":00" and separated by a dash under an "Hours" caption (so `7:00 – 23:00` reads as a time range), and the price per kWh (final, with discounts, fees and taxes included); an error line that wraps; Cancel and Save. Hours are whole numbers 0–23 and prices accept a dot or a comma (checked in `format.js`); Rust checks the rest (day and night must not overlap and must cover the day). Save pipes the JSON to `wattcost config set` (a second Save while one runs is ignored, and closing the menu does not cancel a running save), refreshes the chart and closes the card; cancelled loads show no error; new prices apply to new rows. Entries and buttons get a slight dark shade because themes paint them in nearly the card's colour. Closing the menu closes the card. The card reports no preferred width of its own (compact entries: currency 3em, hours 1.6em, price 3.4em; the price column, captioned "Per kWh", takes the spare width and aligns its content to the right edge in line with Save, which leaves the gap between the hours and the price; captions never ellipsize), so it takes the width the chart gives the menu and opening it never changes the menu's width.
6. No `prefs.js`, no GSettings schema, no stylesheet.
7. Installing and updating: the binary embeds the extension's files, and `wattcost extension install` (also run by `wattcost setup` on GNOME; `gnome-extension/install.sh` runs it from a checkout) writes them to `~/.local/share/gnome-shell/extensions/<uuid>`. GNOME caches extension modules by URL for the whole session, and `ReloadExtension` re-imports the same URLs. The installer therefore copies the modules into a new `build-<timestamp>/` directory, records it in `current-build`, disables the extension, waits until the shell reports it inactive (the shell applies a disable asynchronously, and enabling before that is a no-op; the D-Bus `ReloadExtension` method is not implemented in GNOME 50) and enables it again; `extension.js` imports `indicator.js` from that directory, so the new code loads. Without `current-build` (a packaged install) the modules are imported from the extension directory itself. A brand-new install still needs one log-out (Wayland).

## Testing

1. Rust: unit tests for bucket building (hour and day buckets, DST-length days, empty buckets, power averages, currency fallback) and a CLI JSON shape test.
2. Extension: pure helpers (`format.js`: money and watt formatting, chart scaling) tested with `gjs -m`; the reload path checked in a headless shell (a module changed and reinstalled while the shell runs is picked up).
3. Visual check: a headless GNOME Shell in a separate D-Bus session with a temporary home, the extension enabled there, the menu opened and a screenshot taken; the owner's session is not touched. A new extension appears in the owner's session after logging out and in (Wayland).
