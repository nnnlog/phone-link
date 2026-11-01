#!/usr/bin/env python3
"""Checks the engine's module structure and prints one line per violation; exits 1 on any.

Layering between crates is Cargo's: a crate names only the crates its `Cargo.toml` lists, and the
dependency graph has no cycles. Within a crate that holds several layers, every top-level module has a
rank, and a module may name (`crate::<module>`, in code, not in comments) only modules of a lower rank,
or of the same rank when the pair is listed in SIDEWAYS.

Options:
  --report     print the violations but exit 0
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ENGINE = Path(__file__).resolve().parent.parent

# The ranks of the top-level modules of each crate that holds several layers, keyed by the crate's
# `src/` directory relative to the workspace root.
RANKS = {
    "crates/core/src": {
        "types": 0, "time": 0, "sync": 0,
        "constants": 1, "net": 1, "tap": 1, "logging": 1,
        "config": 2, "codec": 2,
        "edge": 3,
        "identification": 4, "seams": 4,
    },
    "crates/transport/src": {
        "auth": 5, "dcg": 5, "relay": 5,
        "platform": 6,
    },
    "src": {
        "features": 8,
        "engine": 9,
        "api": 10,
        # Test-only modules may name anything.
        "invariants": 99,
    },
}

# Same-rank pairs that may name each other in the given direction.
SIDEWAYS: set[tuple[str, str]] = {
    ("relay", "dcg"),
    ("dcg", "relay"),
}

PATH = re.compile(r"\bcrate::([a-z_][a-z0-9_]*)")


def code_lines(text: str):
    """(line number, line) for every line that is not a comment."""
    for number, line in enumerate(text.splitlines(), 1):
        stripped = line.lstrip()
        if stripped.startswith("//"):
            continue
        yield number, line.split("//")[0] if "//" in line and '"' not in line else line


def check_ranks(src: Path, rank: dict[str, int]) -> list[str]:
    violations = []
    for path in sorted(src.rglob("*.rs")):
        top = path.relative_to(src).parts[0].removesuffix(".rs")
        if top in ("bin", "lib"):
            continue
        rel = path.relative_to(ENGINE)
        if top not in rank:
            violations.append(f"{rel}: module `{top}` has no rank in scripts/layers.py")
            continue
        for number, line in code_lines(path.read_text()):
            for match in PATH.finditer(line):
                target = match.group(1)
                # A name the crate re-exports from a crate beneath it has no rank here.
                if target == top or target not in rank:
                    continue
                if rank[target] < rank[top] or (top, target) in SIDEWAYS:
                    continue
                violations.append(f"{rel}:{number}: `{top}` (rank {rank[top]}) names `{target}` (rank {rank[target]})")
    return violations


def check() -> list[str]:
    violations = []
    for src, rank in RANKS.items():
        violations += check_ranks(ENGINE / src, rank)
    return violations


def main() -> int:
    args = sys.argv[1:]
    violations = check()
    for v in violations:
        print(v)
    print(f"{len(violations)} violation(s)", file=sys.stderr)
    return 0 if "--report" in args or not violations else 1


if __name__ == "__main__":
    sys.exit(main())
