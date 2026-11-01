#!/usr/bin/env python3
"""Checks the engine's `tracing` calls and prints one line per violation; exits 1 on any.

The app's log writes an event's message as it is and redacts every field whose name is not on the
public list (`crates/core/src/logging/policy.rs`). That is only safe when the message is a constant, so every
`trace!`/`debug!`/`info!`/`warn!`/`error!`/`event!` call in the workspace's libraries has:

- a string literal message with no `{…}` placeholder (`{{` and `}}` are literal braces), and nothing
  after it: values go in fields, where the policy sees their names;
- no field named `message`, which `tracing` would print as the message.

The CLI (`src/bin/`), tests, and core's `logging/` (which renders the app's own lines) are not checked.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ENGINE = Path(__file__).resolve().parent.parent
# The root package's `src/` and every workspace crate's.
ROOTS = [ENGINE / "src", *sorted(ENGINE.glob("crates/*/src"))]

MACRO = re.compile(r"\b(?:tracing::)?(trace|debug|info|warn|error|event)!\(")
PLACEHOLDER = re.compile(r"\{(?!\{)")


def skipped(path: Path, root: Path) -> bool:
    rel = path.relative_to(root)
    return (
        rel.parts[0] in ("bin", "logging")
        or path.name in ("tests.rs", "testing.rs")
        or "testing" in rel.parts
    )


def arguments(text: str, open_paren: int) -> list[str]:
    """The top-level comma-separated arguments of the call whose `(` is at `open_paren`."""
    depth, i, current, args = 0, open_paren, [], []
    while i < len(text):
        c = text[i]
        if c == '"':
            end = i + 1
            while text[end] != '"':
                end += 2 if text[end] == "\\" else 1
            current.append(text[i : end + 1])
            i = end + 1
            continue
        if c in "([{":
            depth += 1
            if depth > 1:
                current.append(c)
        elif c in ")]}":
            depth -= 1
            if depth == 0:
                args.append("".join(current).strip())
                return [a for a in args if a]
            current.append(c)
        elif c == "," and depth == 1:
            args.append("".join(current).strip())
            current = []
        else:
            current.append(c)
        i += 1
    raise SystemExit(f"unterminated macro call at offset {open_paren}")


def check() -> list[str]:
    violations = []
    for root in ROOTS:
        violations += check_root(root)
    return violations


def check_root(root: Path) -> list[str]:
    violations = []
    for path in sorted(root.rglob("*.rs")):
        if skipped(path, root):
            continue
        text = path.read_text()
        for m in MACRO.finditer(text):
            line = text.count("\n", 0, m.start()) + 1
            where = f"{path.relative_to(ENGINE)}:{line}"
            args = arguments(text, m.end() - 1)
            literal = next((k for k, a in enumerate(args) if a.startswith('"')), None)
            fields = args if literal is None else args[:literal]
            for f in fields:
                name = re.match(r"[%?]?\s*([A-Za-z_][\w.]*)", f)
                if name and name.group(1) == "message" and not re.match(r"message\s*:", f):
                    violations.append(f"{where}: a field named `message`")
            if literal is None:
                continue
            if PLACEHOLDER.search(args[literal].replace("{{", "")):
                violations.append(f"{where}: the message has a placeholder")
            if len(args) > literal + 1:
                violations.append(f"{where}: arguments after the message")
    return violations


def main() -> int:
    violations = check()
    for v in violations:
        print(v)
    if violations:
        print(f"{len(violations)} violation(s)", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
