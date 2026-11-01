//! The `phonelink` binary end to end, offline: `decode` prints a seam's JSON and fails on bad input;
//! `status` and `store-keys` run against an empty state directory without contacting anything.

use std::path::Path;
use std::process::{Command, Output};

/// Runs the binary with its state and data under `dir` and `args` after them.
fn phonelink(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_phonelink"))
        .arg("--state-dir")
        .arg(dir.join("state"))
        .arg("--data-dir")
        .arg(dir.join("data"))
        .args(args)
        .env_remove("PHONELINK_STATE_DIR")
        .env_remove("PHONELINK_DATA_DIR")
        .output()
        .expect("run phonelink")
}

fn stdout(out: &Output) -> String {
    String::from_utf8(out.stdout.clone()).expect("UTF-8 stdout")
}

/// An inner envelope with `_route: /t`, `_requestId: 7` and a `ValueSet` `{ a: "b" }`.
const ENVELOPE_HEX: &str = "01 27000000 02000000 06000000 5F726F757465 02000000 2F74 0A000000 5F72657175657374 4964 \
                            01000000 37 0C00000000000000 0A0A 0A0161 1205 080C 6A0162";

#[test]
fn decode_prints_the_envelope_as_json() {
    let dir = tempfile::tempdir().unwrap();
    let out = phonelink(dir.path(), &["decode", "envelope", ENVELOPE_HEX]);
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("JSON on stdout");
    assert_eq!(json["headers"]["_route"], "/t");
    assert_eq!(json["payload"]["valueset"]["a"]["String"], "b");
}

#[test]
fn decode_fails_on_input_that_is_not_hex() {
    let dir = tempfile::tempdir().unwrap();
    let out = phonelink(dir.path(), &["decode", "envelope", "zz"]);
    assert!(!out.status.success());
    assert!(stdout(&out).is_empty(), "nothing on stdout: {}", stdout(&out));
    assert!(!out.stderr.is_empty(), "the error is reported on stderr");
}

#[test]
fn status_of_an_empty_state_directory_reports_no_account() {
    let dir = tempfile::tempdir().unwrap();
    let out = phonelink(dir.path(), &["status"]);
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let text = stdout(&out);
    assert!(text.contains("account: not provisioned"), "{text}");
    assert!(text.contains("trust relationships: 0"), "{text}");
}

#[test]
fn store_keys_lists_only_the_identifiers_a_status_minted() {
    let dir = tempfile::tempdir().unwrap();
    assert!(phonelink(dir.path(), &["status"]).status.success());
    let out = phonelink(dir.path(), &["store-keys"]);
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let keys: Vec<String> = stdout(&out).lines().map(str::to_string).collect();
    assert_eq!(keys, ["app/installation-id", "auth/logical-device-id"]);
}
