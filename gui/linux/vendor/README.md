# Vendored dependencies

`cxx-qt/` is a git submodule of [cxx-qt](https://github.com/KDAB/cxx-qt) at v0.10.0. `../Cargo.toml` builds
its `qt-build-utils` crate in place of the one on crates.io (`[patch.crates-io]`), with the patches in this
folder applied. `prepare.sh` checks the submodule out and applies them; `just linux-prepare`, the release
workflow and the Arch PKGBUILD run it before every build.

## Why

At build time cxx-qt compiles the QML with qmlcachegen. By default qmlcachegen also turns the QML into C++
that calls Qt's private QML API (`QQmlPrivate::AOTCompiledContext`). Private API has no compatibility
between Qt versions, so Debian and Ubuntu packages that use it depend on the exact Qt version they were
built with (`qt6-declarative-private-abi (= 6.10.2)`), and Fedora's Qt updates within a release would
break such a build.

`qml-bytecode.patch` passes `--only-bytecode` to qmlcachegen, which leaves the QML as bytecode and the
binary with Qt's public API only. One build on Debian 12 then runs on every distribution with Qt 6.4 or
later. cxx-qt 0.10.0 has no setting for qmlcachegen's arguments, and runs it with an empty environment,
so the change has to be a patch.

## Without the patches

cargo still builds, with no error, but the binary runs only on the Qt version it was built with. Run
`just linux-prepare` (or `prepare.sh`) before building with cargo directly.

When cxx-qt offers a way to pass `--only-bytecode`, use it and remove this folder and the
`[patch.crates-io]` entry.
