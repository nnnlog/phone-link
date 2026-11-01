#!/bin/bash
# Builds the Rust engine (engine/) for the Xcode configuration and regenerates its uniffi Swift
# bindings into Generated/. A generated file is replaced only when it changed, so an unchanged engine
# recompiles nothing on the Swift side.
#
# The engine is a workspace: the phonelink-engine package links every crate beneath it into one static
# library, and each crate that exports uniffi items has its own bindings (`<crate>.swift`,
# `<crate>FFI.h`). All of them compile into the one PhoneLinkEngine Swift module, and their modulemaps
# are joined into Generated/phonelink_engineFFI.modulemap, the one file the Swift flags name.
# BINDINGS lists the crates; the Xcode project compiles exactly these files, so a crate that starts
# or stops exporting uniffi items fails here until both lists change.
#
# Xcode runs this as the first build phase of the PhoneLinkEngine target with ENGINE_PROFILE set by the
# configuration (Debug: debug, Release: release). It also runs by hand from any directory.
#
# The package's manifest lists only the `lib` crate type, so `cargo test` and `cargo build` do not link a
# static library they do not use; this script asks for one with `cargo rustc --crate-type staticlib`.
# The library builds in target/app and the bindgen binary in target/bindgen. Cargo names a static
# library without a hash, so a static library built with other features in the same target directory
# would replace the one the app links.
#
# Cargo runs with a clean environment: Xcode exports SDKROOT, MACOSX_DEPLOYMENT_TARGET, CC and friends,
# which would otherwise change the build with Xcode's settings.
set -euo pipefail

gui_dir="$(cd "$(dirname "$0")/.." && pwd)"
engine_dir="$(cd "$gui_dir/../engine" && pwd)"
profile="${ENGINE_PROFILE:-debug}"
out_dir="$gui_dir/Generated"
BINDINGS=(
    phonelink_engine
    phonelink_core
    phonelink_features
    phonelink_phone_apps
    phonelink_device_status
    phonelink_bluetooth
    phonelink_hotspot
    phonelink_screen_mirroring
    phonelink_clipboard
    phonelink_connected_camera
    phonelink_messaging
    phonelink_remote_control
    phonelink_wallpaper
)

cargo_env=(env -i HOME="$HOME" USER="${USER:-}" PATH="$HOME/.cargo/bin:/usr/bin:/bin:/usr/sbin:/sbin")
if ! "${cargo_env[@]}" which cargo >/dev/null; then
    echo "error: cargo is not on PATH or in ~/.cargo/bin" >&2
    exit 1
fi

cd "$engine_dir"
case "$profile" in
    debug) "${cargo_env[@]}" cargo rustc -p phonelink-engine --lib --crate-type staticlib --target-dir target/app ;;
    release) "${cargo_env[@]}" cargo rustc -p phonelink-engine --lib --crate-type staticlib --release --target-dir target/app ;;
    *)
        echo "error: ENGINE_PROFILE must be debug or release, not '$profile'" >&2
        exit 1
        ;;
esac

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
"${cargo_env[@]}" cargo run --quiet -p phonelink-engine --target-dir target/bindgen --features bindgen-cli --bin uniffi-bindgen -- \
    generate --library "target/app/$profile/libphonelink_engine.a" --language swift --out-dir "$tmp"

for written in "$tmp"/*.swift; do
    name="$(basename "$written" .swift)"
    if [[ ! " ${BINDINGS[*]} " == *" $name "* ]]; then
        echo "error: uniffi-bindgen wrote bindings for $name, which BINDINGS and the Xcode project do not list" >&2
        exit 1
    fi
done

joined="$tmp/joined.modulemap"
: >"$joined"
mkdir -p "$out_dir"
for name in "${BINDINGS[@]}"; do
    for f in "$name.swift" "${name}FFI.h" "${name}FFI.modulemap"; do
        if [ ! -f "$tmp/$f" ]; then
            echo "error: uniffi-bindgen did not write $f" >&2
            exit 1
        fi
    done
    cat "$tmp/${name}FFI.modulemap" >>"$joined"
    printf '\n' >>"$joined"
    for f in "$name.swift" "${name}FFI.h"; do
        if ! cmp -s "$tmp/$f" "$out_dir/$f"; then
            cp "$tmp/$f" "$out_dir/$f"
        fi
    done
done
if ! cmp -s "$joined" "$out_dir/phonelink_engineFFI.modulemap"; then
    cp "$joined" "$out_dir/phonelink_engineFFI.modulemap"
fi
