//! The sync anchors the phone resumes from.

use crate::consts::anchor;
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::HashSet;

#[cfg(feature = "test-support")]
/// A stored anchor row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnchorRow {
    pub content_type: String,
    pub current: i64,
    pub expected: i64,
}

#[cfg(feature = "test-support")]
pub fn anchors(c: &Connection) -> rusqlite::Result<Vec<AnchorRow>> {
    let mut stmt = c.prepare("SELECT content_type, current, expected FROM anchor ORDER BY content_type")?;
    stmt.query_map([], |r| Ok(AnchorRow { content_type: r.get(0)?, current: r.get(1)?, expected: r.get(2)? }))?
        .collect()
}

/// The stored `current` anchor of `content_type`, `-1` when none is stored.
pub fn current_anchor(c: &Connection, content_type: &str) -> rusqlite::Result<i64> {
    Ok(c.query_row("SELECT current FROM anchor WHERE content_type = ?1", [content_type], |r| r.get(0))
        .optional()?
        .unwrap_or(-1))
}

/// The connect response's `expected` anchor of one type (a new row has `current = -1`).
pub fn set_expected_anchor(c: &Connection, content_type: &str, expected: i64) -> rusqlite::Result<()> {
    c.execute(
        "INSERT INTO anchor (content_type, expected) VALUES (?1, ?2)
         ON CONFLICT (content_type) DO UPDATE SET expected = excluded.expected",
        params![content_type, expected],
    )
    .map(|_| ())
}

/// Stores an anchor set: deletes every stored anchor whose type is not in `set`, then stores
/// `current` for each type in `set` (a new row has `expected = -1`).
pub fn complete_anchors(c: &Connection, set: &[(&'static str, i64)]) -> rusqlite::Result<()> {
    let keep: HashSet<&str> = set.iter().map(|(t, _)| *t).collect();
    for t in anchor::ALL {
        if !keep.contains(t) {
            c.execute("DELETE FROM anchor WHERE content_type = ?1", [t])?;
        }
    }

    // Rows of types this build does not know are deleted too.
    let unknown: Vec<String> = {
        let mut stmt = c.prepare("SELECT content_type FROM anchor")?;
        stmt.query_map([], |r| r.get::<_, String>(0))?.collect::<rusqlite::Result<_>>()?
    };
    for t in unknown.iter().filter(|t| !keep.contains(t.as_str())) {
        c.execute("DELETE FROM anchor WHERE content_type = ?1", [t])?;
    }

    for (t, current) in set {
        c.execute(
            "INSERT INTO anchor (content_type, current) VALUES (?1, ?2)
             ON CONFLICT (content_type) DO UPDATE SET current = excluded.current",
            params![t, current],
        )?;
    }
    Ok(())
}

/// Up to date ⇔ every stored anchor has `current ≥ expected`.
pub fn is_up_to_date(c: &Connection) -> rusqlite::Result<bool> {
    let behind: bool =
        c.query_row("SELECT EXISTS (SELECT 1 FROM anchor WHERE current < expected)", [], |r| r.get(0))?;
    Ok(!behind)
}
