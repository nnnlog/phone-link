#!/bin/bash
# Brings PhoneLink/Localizable.xcstrings up to date with the strings the app's code uses: adds new keys,
# marks keys no longer used as stale, and keeps every translation. Reads the .stringsdata files the Swift
# compiler writes for the PhoneLink and PhoneLinkKit targets (SWIFT_EMIT_LOC_STRINGS), so run it after a
# Debug build into the given DerivedData.
#
#     sync-strings.sh <DerivedData dir>
#
# The catalog belongs to the app target, but PhoneLinkKit is a static library compiled as its own target,
# and Xcode syncs a catalog only from its own target's strings (and `xcodebuild` never syncs it). This
# script reads both targets, so it is the catalog's one source of truth.
set -euo pipefail

if [ $# -ne 1 ]; then
    echo "usage: $0 <DerivedData dir>" >&2
    exit 1
fi
gui="$(cd "$(dirname "$0")/.." && pwd)"
intermediates="$1/Build/Intermediates.noindex/PhoneLink.build/Debug"
catalog="$gui/PhoneLink/Localizable.xcstrings"

files=()
for target in PhoneLink PhoneLinkKit; do
    dir="$intermediates/$target.build"
    if [ ! -d "$dir" ]; then
        echo "error: $dir does not exist; build Debug first" >&2
        exit 1
    fi
    while IFS= read -r f; do files+=("$f"); done < <(find "$dir" -name '*.stringsdata' ! -name 'ExtractedAppShortcutsMetadata.stringsdata')
done
if [ ${#files[@]} -eq 0 ]; then
    echo "error: no .stringsdata under $intermediates; is SWIFT_EMIT_LOC_STRINGS on?" >&2
    exit 1
fi

xcrun xcstringstool sync "$catalog" --stringsdata "${files[@]}"
echo "synced $catalog from ${#files[@]} files"
