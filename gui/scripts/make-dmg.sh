#!/bin/bash
# Packages a built PhoneLink.app as a drag-to-install disk image: the app beside a link to /Applications,
# on an APFS volume, LZFSE-compressed (ULFO), named <app name>-<CFBundleShortVersionString>.dmg. Uses
# `diskutil image`, which replaces the deprecated `hdiutil create`.
#
#     make-dmg.sh <path/to/PhoneLink.app> <output dir>
#
# The image itself is not signed; the app keeps its signature, and its notarization ticket when
# notarize.sh exported it. Without one, Gatekeeper on another Mac blocks the first launch until the user
# allows it.
set -euo pipefail

if [ $# -ne 2 ]; then
    echo "usage: $0 <path/to/App.app> <output dir>" >&2
    exit 1
fi
app="$1"
out_dir="$2"
if [ ! -d "$app" ]; then
    echo "error: $app does not exist; build the Release app first" >&2
    exit 1
fi

info="$app/Contents/Info.plist"
name="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleName' "$info")"
version="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$info")"
dmg="$out_dir/$name-$version.dmg"

staging="$(mktemp -d)"
trap 'rm -rf "$staging"' EXIT
ditto "$app" "$staging/$(basename "$app")"
ln -s /Applications "$staging/Applications"

mkdir -p "$out_dir"
rm -f "$dmg"
diskutil image create from --format ULFO --volumeName "$name" "$staging" "$dmg" >/dev/null
hdiutil verify -quiet "$dmg"
echo "$dmg"
