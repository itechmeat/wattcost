#!/bin/sh
# Installs the extension for the current user and reloads it in the running session.
set -eu
uuid=wattcost@itechmeat.github.io
if ! command -v gnome-extensions >/dev/null 2>&1; then
    echo "GNOME Shell was not found; the top bar indicator needs GNOME 50. wattcost report shows the numbers."
    exit 0
fi
src=$(cd "$(dirname "$0")" && pwd)
dest=${XDG_DATA_HOME:-$HOME/.local/share}/gnome-shell/extensions/$uuid

mkdir -p "$dest"
# The running shell keeps extension.js cached, so a changed one needs a new login session.
loader_changed=false
[ -f "$dest/extension.js" ] && ! cmp -s "$src/extension.js" "$dest/extension.js" && loader_changed=true

# A fresh directory per install: GNOME caches modules by URL for the whole session.
build=$(basename "$(mktemp -d "$dest/build-XXXXXXXX")")
chmod 755 "$dest/$build"
cp "$src/charts.js" "$src/data.js" "$src/format.js" "$src/indicator.js" "$src/settings.js" "$dest/$build/"
cp "$src/metadata.json" "$src/extension.js" "$dest/"
rm -rf "$dest/icons"
cp -r "$src/icons" "$dest/"
echo "$build" > "$dest/current-build"
find "$dest" -maxdepth 1 -name 'build-*' ! -name "$build" -exec rm -rf {} +
rm -f "$dest/charts.js" "$dest/data.js" "$dest/format.js" "$dest/indicator.js" "$dest/settings.js" "$dest/stylesheet.css"

extension_state() {
    gnome-extensions info "$uuid" 2>/dev/null | sed -n 's/^ *State: //p'
}

# Polls for up to five seconds.
wait_for_state() {
    tries=25
    while [ "$(extension_state)" != "$1" ]; do
        tries=$((tries - 1))
        [ "$tries" -gt 0 ] || return 1
        sleep 0.2
    done
}

state=$(extension_state)
if [ -z "$state" ]; then
    enabled=$(gsettings get org.gnome.shell enabled-extensions)
    case $enabled in
        *"'$uuid'"*) ;;
        *) gsettings set org.gnome.shell enabled-extensions \
            "$(echo "$enabled" | sed "s/@as //; s/]\$/, '$uuid']/; s/\[, /[/")" ;;
    esac
    echo "Installed $uuid; GNOME loads a new extension after you log out and back in."
elif [ "$loader_changed" = true ]; then
    echo "Updated $uuid; extension.js changed, so log out and back in once to load it."
elif [ "$state" = ACTIVE ]; then
    # The shell applies disable asynchronously; enabling before it has done so is a no-op.
    gnome-extensions disable "$uuid"
    wait_for_state INACTIVE
    gnome-extensions enable "$uuid"
    if wait_for_state ACTIVE; then
        echo "Reloaded $uuid."
    else
        echo "Reload of $uuid did not finish; check: gnome-extensions info $uuid" >&2
        exit 1
    fi
else
    echo "Updated $uuid (state: $state); it was not enabled, so nothing was reloaded."
fi
