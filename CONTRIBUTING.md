# Contributing

Thanks for helping. By taking part you agree to the [Code of Conduct](CODE_OF_CONDUCT.md). Report security problems privately as described in [SECURITY.md](SECURITY.md).

## Gates

Every change must pass:

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
gjs -m gnome-extension/tests/format.test.js
code-ranker check .
code-ranker check gnome-extension --plugins js
```

## Adding a provider

1. Create `src/providers/<name>.rs` with a struct implementing `Provider` and a `probe` function returning `ProbeResult`.
2. Read files through `sysfs` helpers and turn counters into deltas with `Counter`.
3. Register it in `detect_with` and add its name to `PROVIDER_NAMES` in `src/config.rs`. Return `false` from `measures_energy` if it only reports utilisation.
4. Test it against a fake tree built with `test_support::FakeFs`.

## GNOME extension

The binary embeds the extension: new modules in `gnome-extension/` must be added to `MODULES` in `src/extension.rs`. `gnome-extension/install.sh` builds the checkout and installs the extension from it, reloading it in the running session. Check visual changes in a headless shell (`gnome-shell --headless --virtual-monitor` in its own `dbus-run-session`) rather than in your own session.

## Style

Small single-purpose modules, English everywhere, comments only for reasons the code cannot show. See `AGENTS.md`.

Contributions are accepted under the project's Apache License 2.0.
