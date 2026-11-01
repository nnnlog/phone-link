//! The crate's own `uniffi-bindgen`:
//! `cargo run --features bindgen-cli --bin uniffi-bindgen -- generate --library <lib> --language swift --out-dir <dir>`.

fn main() {
    uniffi::uniffi_bindgen_main()
}
