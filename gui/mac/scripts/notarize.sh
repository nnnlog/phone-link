#!/bin/bash
# Notarizes an archived app and exports the stapled copy: `xcodebuild -exportArchive` with
# ExportOptions-DeveloperID-Notarize.plist re-signs the archive with Developer ID and uploads it to Apple's
# notary service under the Xcode account (no app-specific password or API key), then
# `xcodebuild -exportNotarizedApp` is retried every 30 seconds until the service has accepted it and writes
# the stapled app to the output directory. Checks the staple and that Gatekeeper accepts the app as
# notarized.
#
#     notarize.sh <path/to/App.xcarchive> <output dir>
#
# The upload is recorded in the archive, so after a timeout running `xcodebuild -exportNotarizedApp`
# with the same archive later picks up the same submission.
set -euo pipefail

if [ $# -ne 2 ]; then
    echo "usage: $0 <path/to/App.xcarchive> <output dir>" >&2
    exit 1
fi
archive="$1"
out_dir="$2"
if [ ! -d "$archive" ]; then
    echo "error: $archive does not exist; archive the Release app first" >&2
    exit 1
fi
export_options="$(dirname "$0")/../ExportOptions-DeveloperID-Notarize.plist"
poll_seconds=30
max_polls=240 # 2 hours

upload_dir="$(mktemp -d)"
log="$(mktemp)"
trap 'rm -rf "$upload_dir" "$log"' EXIT

xcodebuild -exportArchive -archivePath "$archive" -exportPath "$upload_dir" \
    -exportOptionsPlist "$export_options" -allowProvisioningUpdates

# While the service is still processing, the export fails with "... is processing and not ready for
# distribution"; any other failure (a rejection among them) stops here.
for ((poll = 1; ; poll++)); do
    rm -rf "$out_dir"
    if xcodebuild -exportNotarizedApp -archivePath "$archive" -exportPath "$out_dir" >"$log" 2>&1; then
        break
    fi
    if ! grep -q 'is processing and not ready for distribution' "$log"; then
        cat "$log" >&2
        exit 1
    fi
    if [ "$poll" -ge "$max_polls" ]; then
        echo "error: not notarized after $((max_polls * poll_seconds / 60)) minutes; run" \
            "\`xcodebuild -exportNotarizedApp -archivePath $archive -exportPath $out_dir\` later" >&2
        exit 1
    fi
    echo "waiting for notarization ($((poll * poll_seconds / 60)) min)"
    sleep "$poll_seconds"
done

app="$(find "$out_dir" -maxdepth 1 -name '*.app' | head -1)"
if [ -z "$app" ]; then
    echo "error: -exportNotarizedApp wrote no app into $out_dir" >&2
    exit 1
fi
codesign --verify --deep --strict "$app"
xcrun stapler validate "$app"
spctl --assess --type execute --verbose=2 "$app" 2>&1 | tee "$log"
grep -q '^source=Notarized Developer ID$' "$log" || {
    echo "error: Gatekeeper does not accept $app as notarized" >&2
    exit 1
}
echo "$app"
