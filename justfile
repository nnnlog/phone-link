# Every command for the app. Recipes run from this directory whatever the caller's directory.

# Xcode is installed but xcode-select may point at the Command Line Tools; switching it needs sudo.
export DEVELOPER_DIR := env("DEVELOPER_DIR", "/Applications/Xcode.app/Contents/Developer")

xcodebuild := "xcodebuild -project gui/mac/PhoneLink.xcodeproj -scheme PhoneLink -derivedDataPath gui/mac/build/DerivedData"

# List the recipes
default:
    @just --list --unsorted

# Engine and app tests, engine lints, the translations
check: engine-lint engine-test gui-test gui-strings-check

# Build the engine library and the CLI
engine-build:
    cd engine && cargo build --workspace

# Run the engine's offline tests
engine-test:
    cd engine && cargo test --workspace

# Clippy with warnings as errors (with and without the test support), the module layering, the log
# calls' constant messages, and rustfmt in check mode
engine-lint:
    cd engine && cargo clippy --workspace --all-targets -- -D warnings
    cd engine && cargo clippy --workspace --lib --bins -- -D warnings
    python3 -I engine/scripts/layers.py
    python3 -I engine/scripts/log_calls.py
    cd engine && cargo fmt --check

# The engine's module structure: the layers within a crate (engine/scripts/layers.py)
engine-layers *args:
    python3 -I engine/scripts/layers.py {{args}}

# Count the engine's warnings for every lint the workspace Cargo.toml names, including those not yet turned on
engine-audit:
    engine/scripts/lint-audit.sh

# Regenerate the Swift bindings and diff them against engine/target/uniffi-baseline/ (made by
# `just engine-bindings-baseline`); an empty diff means the uniffi surface is unchanged
engine-bindings-diff: engine-bindings
    diff -ru engine/target/uniffi-baseline engine/target/uniffi

# Save the current Swift bindings as the baseline engine-bindings-diff compares against
engine-bindings-baseline: engine-bindings
    rm -rf engine/target/uniffi-baseline
    cp -R engine/target/uniffi engine/target/uniffi-baseline

# Format the engine
engine-fmt:
    cd engine && cargo fmt

# Generate the Swift bindings from the debug static library (engine/target/app/, where the app build
# makes it) into engine/target/uniffi/ (the app build makes its own)
engine-bindings:
    cd engine && cargo rustc -p phonelink-engine --lib --crate-type staticlib --target-dir target/app
    cd engine && cargo run -p phonelink-engine --target-dir target/bindgen --features bindgen-cli --bin uniffi-bindgen -- generate --library target/app/debug/libphonelink_engine.a --language swift --out-dir target/uniffi

# Build the Linux app (gui/linux/); cxx-qt finds Qt 6 through QMAKE
linux-build:
    cd gui/linux && QMAKE="${QMAKE:-qmake6}" cargo build

# Run the Linux app
linux-run:
    cd gui/linux && QMAKE="${QMAKE:-qmake6}" cargo run

# The Linux app's lints (warnings as errors), formatting, tests, and its log calls' constant messages
linux-check:
    cd gui/linux && QMAKE="${QMAKE:-qmake6}" cargo clippy --all-targets -- -D warnings
    cd gui/linux && cargo fmt --check
    cd gui/linux && QMAKE="${QMAKE:-qmake6}" cargo test
    python3 -I engine/scripts/log_calls.py gui/linux/src

# Install the Linux app for this user (~/.local), starting in the tray at login; needs desktop-file-install
linux-install:
    cd gui/linux && QMAKE="${QMAKE:-qmake6}" cargo install --locked --path . --root ~/.local
    desktop-file-install --dir="$HOME/.local/share/applications" --set-key=Exec --set-value="$HOME/.local/bin/phonelink-linux %u" gui/linux/data/phonelink-linux.desktop
    desktop-file-install --dir="$HOME/.config/autostart" --set-key=Exec --set-value="$HOME/.local/bin/phonelink-linux --background" gui/linux/data/phonelink-linux-autostart.desktop
    install -Dm644 gui/linux/data/phonelink.png ~/.local/share/icons/hicolor/512x512/apps/phonelink.png

# Build the Debian package (needs cargo-deb) into gui/linux/target/debian/
linux-deb:
    cd gui/linux && QMAKE="${QMAKE:-qmake6}" cargo deb

# Build the RPM package (needs cargo-generate-rpm) into gui/linux/target/generate-rpm/
linux-rpm:
    cd gui/linux && QMAKE="${QMAKE:-qmake6}" cargo build --release
    strip -s gui/linux/target/release/phonelink-linux
    cd gui/linux && cargo generate-rpm

# Run the engine CLI, e.g. `just cli decode envelope @file`
cli *args:
    cd engine && cargo run --bin phonelink -- {{args}}

# Build the app (Debug or Release); the build also builds the engine and regenerates the bindings
gui-build config="Debug":
    {{xcodebuild}} -configuration {{config}} build

# Run the app's unit tests
gui-test:
    {{xcodebuild}} -configuration Debug test

# Build Debug and bring gui/mac/PhoneLink/Localizable.xcstrings up to date with the strings the app and
# PhoneLinkKit use (scripts/sync-strings.sh); translations are kept
gui-strings: gui-build
    gui/mac/scripts/sync-strings.sh gui/mac/build/DerivedData

# Fail when a string catalog lacks a translation for a language in the project's knownRegions
gui-strings-check:
    gui/mac/scripts/check-strings.sh

# This Mac's build: signed as Signing.local.xcconfig names, for this Mac (and the team's registered
# devices). `distribute` makes the build for other Macs.

# Build the Release app for this Mac
build-release: (gui-build "Release")

release_app := "gui/mac/build/DerivedData/Build/Products/Release/PhoneLink.app"

# Build the Release app for this Mac and package it as gui/mac/build/PhoneLink-<version>.dmg
dmg: build-release
    gui/mac/scripts/make-dmg.sh {{release_app}} gui/mac/build

# Install the dmg `just dmg` made for the current Release build over /Applications/PhoneLink.app, quitting
# and relaunching a running copy; builds nothing
install:
    gui/mac/scripts/install-dmg.sh "gui/mac/build/PhoneLink-$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' {{release_app}}/Contents/Info.plist).dmg"

release_archive := "gui/mac/build/PhoneLink.xcarchive"
distribution_dir := "gui/mac/build/developer-id"
distribution_app := distribution_dir / "PhoneLink.app"

# The build for other Macs: archives the Release app (signed as for this Mac), has Xcode re-sign the archive
# with the team's cloud-managed Developer ID certificate and a Developer ID profile it makes itself, and
# packages that as a dmg. Needs Signing.local.xcconfig's team and an Xcode account on that team with
# access to cloud-managed Developer ID certificates. Not notarized. Raise CURRENT_PROJECT_VERSION in
# gui/mac/Version.xcconfig for each one, or a Mac that has the camera extension keeps its older copy.
# Archive, sign with Developer ID and package the app for other Macs as gui/mac/build/developer-id/PhoneLink-<version>.dmg
distribute:
    {{xcodebuild}} -configuration Release archive -archivePath {{release_archive}}
    rm -rf {{distribution_dir}}
    xcodebuild -exportArchive -archivePath {{release_archive}} -exportPath {{distribution_dir}} -exportOptionsPlist gui/mac/ExportOptions-DeveloperID.plist -allowProvisioningUpdates
    codesign --verify --deep --strict {{distribution_app}}
    codesign -dvv {{distribution_app}} 2>&1 | grep -q '^Authority=Developer ID Application: ' || { echo "error: {{distribution_app}} is not signed with a Developer ID certificate" >&2; exit 1; }
    gui/mac/scripts/make-dmg.sh {{distribution_app}} {{distribution_dir}}

notarized_dir := "gui/mac/build/notarized"

# `distribute`, notarized: archives the Release app, has Xcode re-sign it with Developer ID and upload it to
# Apple's notary service under the Xcode account, waits for the stapled app (minutes to an hour or more) and
# packages that as a dmg (scripts/notarize.sh). Gatekeeper on other Macs opens it without asking to allow
# it. Same needs and version rule as `distribute`.
# Archive, notarize and package the app for other Macs as gui/mac/build/notarized/PhoneLink-<version>.dmg
distribute-notarized:
    {{xcodebuild}} -configuration Release archive -archivePath {{release_archive}}
    gui/mac/scripts/notarize.sh {{release_archive}} {{notarized_dir}}
    gui/mac/scripts/make-dmg.sh {{notarized_dir}}/PhoneLink.app {{notarized_dir}}

ictool := DEVELOPER_DIR / "../Applications/Icon Composer.app/Contents/Executables/ictool"

# Regenerate the icons from gui/mac/scripts/make_icons.py, and the README's Liquid Glass rendering of the app icon
icons:
    python3 -I gui/mac/scripts/make_icons.py
    "{{ictool}}" gui/mac/PhoneLink/App/AppIcon.icon --export-image --output-file gui/mac/Design/AppIcon.png --platform macOS --rendition Default --width 256 --height 256 --scale 2 >/dev/null
