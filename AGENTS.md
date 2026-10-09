# Agent instructions

wattcost is a Rust CLI that logs a computer's power draw and prices it with a tariff, plus a GNOME Shell extension in `gnome-extension/` that shows the result in the top bar (keep it on system widgets and theme colours). Read `docs/superpowers/specs/` before changing behaviour.

## Rules

1. English only in code, docs, comments and messages.
2. Keep modules small and single-purpose. The wall-energy formula (`src/energy.rs`), the price formula (`src/tariff.rs`) and counter deltas (`src/providers/counter.rs`) exist once; reuse them.
3. Comments explain why, never what. No commented-out code.
4. Library errors are `thiserror` enums; `anyhow` is used only in `src/cli/`.
5. `unsafe` is forbidden.
6. Nothing machine- or user-specific goes into the repository: real prices, locations, host names and paths belong in the user's config.
7. Follow the `rust-skills` skill in `.agents/skills/rust-skills`.

## Tools

1. Semantic code search: `zg query "<question>"` (index: `ZVEC_GREP_EMBEDDING=local/potion-code-16m-v2 ZVEC_GREP_DEVICE=cpu zg index --mode direct`). Exact strings: `rg`.
2. Structure gate: `code-ranker check .` must report no violations before work is handed over.

## Gates

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
gjs -m gnome-extension/tests/format.test.js
code-ranker check .
code-ranker check gnome-extension --plugins js
```
