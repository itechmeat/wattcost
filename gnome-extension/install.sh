#!/bin/sh
# Installs the extension from this checkout; the logic lives in `wattcost extension install`.
set -eu
exec cargo run --quiet --manifest-path "$(dirname "$0")/../Cargo.toml" -- extension install
