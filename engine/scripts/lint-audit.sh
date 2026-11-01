#!/bin/bash
# Prints the number of warnings per lint for every lint the workspace's `Cargo.toml` lists, whatever its
# level, so the lints not yet turned on can be burned down. Builds into target/lint-audit/ to keep the
# normal build's cache.
set -euo pipefail
cd "$(dirname "$0")/.."
lints=$(sed -n '/^\[workspace.lints.clippy\]/,/^\[/p' Cargo.toml | sed -nE 's/^([a-z_]+) = .*/\1/p')
args=()
for l in $lints; do args+=(-W "clippy::$l"); done
# Every crate's root, so clippy re-checks each one rather than replaying a cached result.
touch src/lib.rs crates/*/src/lib.rs
cargo clippy --workspace --all-targets --target-dir target/lint-audit --message-format=json -- "${args[@]}" -W unreachable_pub -W unused_qualifications -W missing_docs 2>/dev/null |
    python3 -I -c '
import json, sys, collections
seen, counts = set(), collections.Counter()
for line in sys.stdin:
    try:
        m = json.loads(line)
    except ValueError:
        continue
    msg = m.get("message") or {}
    if m.get("reason") != "compiler-message" or msg.get("level") != "warning" or not msg.get("code"):
        continue
    span = msg["spans"][0] if msg["spans"] else {}
    key = (msg["code"]["code"], span.get("file_name"), span.get("line_start"))
    if key not in seen:
        seen.add(key)
        counts[msg["code"]["code"]] += 1
for lint, n in counts.most_common():
    print(f"{n:6} {lint}")
'
