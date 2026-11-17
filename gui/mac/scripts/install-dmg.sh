#!/bin/bash
# Installs the app from a disk image made by make-dmg.sh, as dragging it to /Applications would: quits
# the running copy, replaces /Applications/<App>.app with the image's app, and relaunches it if it was
# running.
#
#     install-dmg.sh <path/to/App-version.dmg>
set -euo pipefail

if [ $# -ne 1 ]; then
    echo "usage: $0 <path/to/App.dmg>" >&2
    exit 1
fi
dmg="$1"
if [ ! -f "$dmg" ]; then
    echo "error: $dmg does not exist; run \`just dmg\` first" >&2
    exit 1
fi

mount="$(mktemp -d)"
detach() {
    hdiutil detach -quiet "$mount" || hdiutil detach -quiet -force "$mount"
    rmdir "$mount"
}
hdiutil attach -quiet -nobrowse -readonly -noautoopen -mountpoint "$mount" "$dmg"
trap detach EXIT

apps=("$mount"/*.app)
if [ ${#apps[@]} -ne 1 ] || [ ! -d "${apps[0]}" ]; then
    echo "error: expected exactly one .app in $dmg" >&2
    exit 1
fi
app="${apps[0]}"
bundle_id="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$app/Contents/Info.plist")"
target="/Applications/$(basename "$app")"

# `tell application id` would launch the app if it were not running, so check first. The app ends its
# phone session before it quits, for at most a few seconds.
running="$(osascript -e "application id \"$bundle_id\" is running")"
if [ "$running" = "true" ]; then
    osascript -e "tell application id \"$bundle_id\" to quit"
    for _ in $(seq 30); do
        [ "$(osascript -e "application id \"$bundle_id\" is running")" = "false" ] && break
        sleep 0.5
    done
    if [ "$(osascript -e "application id \"$bundle_id\" is running")" = "true" ]; then
        echo "error: $bundle_id did not quit within 15 s" >&2
        exit 1
    fi
fi

rm -rf "$target"
ditto "$app" "$target"
echo "$target"

if [ "$running" = "true" ]; then
    open "$target"
fi
