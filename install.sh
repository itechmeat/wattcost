#!/bin/sh
# Installs the latest wattcost release into ~/.local/bin, then prints the next step.
# Usage: curl -fsSL https://raw.githubusercontent.com/itechmeat/wattcost/main/install.sh | sh
set -eu

repo=itechmeat/wattcost
bin_dir=${WATTCOST_BIN_DIR:-$HOME/.local/bin}

case $(uname -s)/$(uname -m) in
    Linux/x86_64) target=x86_64-unknown-linux-gnu ;;
    Linux/aarch64 | Linux/arm64) target=aarch64-unknown-linux-gnu ;;
    *)
        echo "wattcost has release builds for Linux on x86_64 and aarch64 only; try: cargo install wattcost" >&2
        exit 1
        ;;
esac

archive=wattcost-$target.tar.gz
url=https://github.com/$repo/releases/latest/download/$archive
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

echo "Downloading $archive"
curl -fsSL "$url" -o "$work/$archive"
curl -fsSL "$url.sha256" -o "$work/$archive.sha256"
(cd "$work" && sha256sum -c "$archive.sha256" >/dev/null) || {
    echo "The checksum of $archive does not match; nothing was installed." >&2
    exit 1
}

tar -xzf "$work/$archive" -C "$work" wattcost
mkdir -p "$bin_dir"
install -m 0755 "$work/wattcost" "$bin_dir/wattcost"
echo "Installed $bin_dir/wattcost ($("$bin_dir/wattcost" --version))"

case :$PATH: in
    *:"$bin_dir":*) ;;
    *) echo "Add $bin_dir to PATH, for example: echo 'export PATH=\"$bin_dir:\$PATH\"' >> ~/.profile" ;;
esac
echo "Next: set your prices with 'wattcost config set' (see README), then run 'wattcost setup'."
