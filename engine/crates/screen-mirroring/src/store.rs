//! The per-phone settings of mirroring and the per-package values it keeps (the last screen layout of
//! an app window, the app's volume), persisted in the phone's store.

use rusqlite::OptionalExtension;

use phonelink_features::shared::db::{FeatureDb, StoreError};

/// The feature's name: its store.
pub const FEATURE: &str = "screen_mirroring";

pub const MIGRATIONS: &[&str] = &[
    "CREATE TABLE settings (
        id INTEGER PRIMARY KEY CHECK (id = 0),
        allow_apps INTEGER NOT NULL,
        black_screen INTEGER NOT NULL,
        audio_streaming INTEGER NOT NULL,
        always_expanded INTEGER NOT NULL,
        do_not_ask_restart INTEGER NOT NULL,
        do_not_ask_task_transfer INTEGER NOT NULL,
        do_not_show_true_touch INTEGER NOT NULL,
        do_not_ask_skip_unlock INTEGER NOT NULL,
        do_not_ask_skip_verification INTEGER NOT NULL,
        do_not_ask_switch_layout INTEGER NOT NULL
    );",
    "CREATE TABLE packages (
        package_name TEXT PRIMARY KEY NOT NULL,
        screen_layout INTEGER NOT NULL,
        volume INTEGER NOT NULL,
        muted INTEGER NOT NULL
    );",
];

/// The phone's mirroring settings on this Mac.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settings {
    /// "Allow apps": off, no window starts.
    pub allow_apps: bool,
    /// Blank the phone's own screen while it is mirrored.
    pub black_screen: bool,
    /// Play the phone's app audio on the Mac.
    pub audio_streaming: bool,
    /// Open app windows in the expanded layout.
    pub always_expanded: bool,
    /// Answer the phone's "move to the Mac" request without asking (app restart / task transfer).
    pub do_not_ask_restart: bool,
    pub do_not_ask_task_transfer: bool,
    /// Skip the phone's touch-confirmation screen.
    pub do_not_show_true_touch: bool,
    /// Nearby: continue without asking to skip unlocking / verification.
    pub do_not_ask_skip_unlock: bool,
    pub do_not_ask_skip_verification: bool,
    /// Switch an app window's screen layout without asking.
    pub do_not_ask_switch_layout: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            allow_apps: true,
            black_screen: true,
            audio_streaming: true,
            always_expanded: false,
            do_not_ask_restart: false,
            do_not_ask_task_transfer: false,
            do_not_show_true_touch: false,
            do_not_ask_skip_unlock: false,
            do_not_ask_skip_verification: false,
            do_not_ask_switch_layout: false,
        }
    }
}

/// One package's stored values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PackageData {
    /// The last screen layout of its app window (`ScreenLayout`).
    pub screen_layout: i32,
    /// 0–100.
    pub volume: i32,
    pub muted: bool,
}

impl Default for PackageData {
    fn default() -> Self {
        Self { screen_layout: 0, volume: 50, muted: false }
    }
}

type SettingsRow = (bool, bool, bool, bool, bool, bool, bool, bool, bool, bool);

pub(crate) fn load_settings(db: &FeatureDb) -> Result<Settings, StoreError> {
    let row: Option<SettingsRow> = db.read(|c| {
        c.query_row(
            "SELECT allow_apps, black_screen, audio_streaming, always_expanded, do_not_ask_restart,
                    do_not_ask_task_transfer, do_not_show_true_touch, do_not_ask_skip_unlock,
                    do_not_ask_skip_verification, do_not_ask_switch_layout
             FROM settings WHERE id = 0",
            [],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                    r.get(7)?,
                    r.get(8)?,
                    r.get(9)?,
                ))
            },
        )
        .optional()
    })?;

    Ok(match row {
        None => Settings::default(),
        Some((a, b, c, d, e, f, g, h, i, j)) => Settings {
            allow_apps: a,
            black_screen: b,
            audio_streaming: c,
            always_expanded: d,
            do_not_ask_restart: e,
            do_not_ask_task_transfer: f,
            do_not_show_true_touch: g,
            do_not_ask_skip_unlock: h,
            do_not_ask_skip_verification: i,
            do_not_ask_switch_layout: j,
        },
    })
}

pub(crate) fn save_settings(db: &FeatureDb, s: &Settings) -> Result<(), StoreError> {
    db.transaction(|tx| -> Result<(), StoreError> {
        tx.execute(
            "INSERT INTO settings (id, allow_apps, black_screen, audio_streaming, always_expanded,
                 do_not_ask_restart, do_not_ask_task_transfer, do_not_show_true_touch,
                 do_not_ask_skip_unlock, do_not_ask_skip_verification, do_not_ask_switch_layout)
             VALUES (0, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT (id) DO UPDATE SET allow_apps = excluded.allow_apps,
                 black_screen = excluded.black_screen, audio_streaming = excluded.audio_streaming,
                 always_expanded = excluded.always_expanded,
                 do_not_ask_restart = excluded.do_not_ask_restart,
                 do_not_ask_task_transfer = excluded.do_not_ask_task_transfer,
                 do_not_show_true_touch = excluded.do_not_show_true_touch,
                 do_not_ask_skip_unlock = excluded.do_not_ask_skip_unlock,
                 do_not_ask_skip_verification = excluded.do_not_ask_skip_verification,
                 do_not_ask_switch_layout = excluded.do_not_ask_switch_layout",
            rusqlite::params![
                s.allow_apps,
                s.black_screen,
                s.audio_streaming,
                s.always_expanded,
                s.do_not_ask_restart,
                s.do_not_ask_task_transfer,
                s.do_not_show_true_touch,
                s.do_not_ask_skip_unlock,
                s.do_not_ask_skip_verification,
                s.do_not_ask_switch_layout
            ],
        )
        .map(|_| ())
        .map_err(StoreError::from)
    })
}

pub(crate) fn load_package(db: &FeatureDb, package: &str) -> Result<PackageData, StoreError> {
    let row: Option<(i32, i32, bool)> = db.read(|c| {
        c.query_row("SELECT screen_layout, volume, muted FROM packages WHERE package_name = ?1", [package], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .optional()
    })?;
    Ok(row.map_or_else(PackageData::default, |(screen_layout, volume, muted)| PackageData {
        screen_layout,
        volume,
        muted,
    }))
}

pub(crate) fn save_package(db: &FeatureDb, package: &str, data: &PackageData) -> Result<(), StoreError> {
    db.transaction(|tx| -> Result<(), StoreError> {
        tx.execute(
            "INSERT INTO packages (package_name, screen_layout, volume, muted) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (package_name) DO UPDATE SET screen_layout = excluded.screen_layout,
                 volume = excluded.volume, muted = excluded.muted",
            rusqlite::params![package, data.screen_layout, data.volume, data.muted],
        )
        .map(|_| ())
        .map_err(StoreError::from)
    })
}
