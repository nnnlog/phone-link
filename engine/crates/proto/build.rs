//! Compiles every schema under `proto/` with protox + prost-build (no `protoc`). `proto/` is the one
//! import root: each file sits at its package path and imports others by it
//! (`platform_sdk/v1/trace_context.proto`).

use std::path::{Path, PathBuf};

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("read_dir {}: {e}", dir.display()))
        .map(|e| e.expect("dir entry").path())
        .collect();
    entries.sort();

    for path in entries {
        if path.is_dir() {
            walk(&path, out);
        } else if path.extension().is_some_and(|x| x == "proto") {
            out.push(path);
        }
    }
}

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let root = manifest.join("proto");
    println!("cargo:rerun-if-changed={}", root.display());

    let mut files = Vec::new();
    walk(&root, &mut files);
    assert!(!files.is_empty(), "no .proto files under {}", root.display());

    let mut compiler = protox::Compiler::new([&root]).unwrap_or_else(|e| panic!("{e}"));
    compiler.include_imports(true);
    compiler.open_files(&files).unwrap_or_else(|e| panic!("{e:?}"));
    for file in compiler.files() {
        if let Some(path) = file.path() {
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }

    // The google/protobuf/* imports stay in the set: prost-build maps them to prost_types and needs
    // their descriptors to resolve the fields that use them.
    prost_build::Config::new()
        .include_file("_includes.rs")
        .compile_fds(compiler.file_descriptor_set())
        .unwrap_or_else(|e| panic!("prost-build: {e}"));
}
