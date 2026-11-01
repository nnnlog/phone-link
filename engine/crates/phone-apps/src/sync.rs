//! The inbound `phoneApps` processor: the metadata message, the phone-app and recent-apps item
//! messages, completion and the sequence numbers.
//!
//! A metadata message reconciles both tables against the phone's id lists in one transaction and the
//! reply names the ids the PC needs. The processor then stays in the App layer's unfinished-processor
//! table under the sync's key while a phone-app id is pending; each item message removes the ids it
//! carries. An item message with no open sync is processed on its own (an incremental push). A message
//! is validated whole before anything is applied; a refused one changes nothing — not even the remoting
//! capabilities its `phoneAppsSettings` reports — and is answered `{ result: 1, message }`.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use async_trait::async_trait;
use rusqlite::Transaction;
use tracing::{error, info, warn};

use phonelink_app::consts::key as app_key;
use phonelink_app::{ContentProcessor, ContentProcessorFactory, InboundContext, Processed, Rejection, simple_answer};
use phonelink_core::codec::valueset::ValueSet;

use super::store::{self, Content, RecentRow};
use super::wire::{self, AppBatch, Items, RecentBatch};
use super::{Inner, PhoneAppsEvent, PhoneState};

/// A refused message: `{ result: Int32 1, message: String "<reason> 80004005" }`.
fn rejection(reason: &'static str) -> Rejection {
    Rejection::with_answer(reason, wire::rejection_answer(reason))
}

/// Stores a phone-app batch: its upserts (keeping each package's favorite rank), then its deletes.
fn store_apps(tx: &Transaction<'_>, batch: &AppBatch, favorites: &HashMap<String, i64>) -> rusqlite::Result<()> {
    let (deletes, upserts): (Vec<_>, Vec<_>) = batch.items.iter().partition(|i| batch.with_actions && i.delete);
    for item in upserts {
        let rank = store::kept_rank(tx, favorites, &item.package_name)?;
        store::upsert_app(tx, item, rank)?;
    }

    let ids: Vec<i64> = deletes.iter().map(|i| i.id).collect();
    store::delete_apps(tx, &ids)?;
    Ok(())
}

/// Stores a recent-apps batch whose entries' desktop URIs are `desktop`, removing each stored entry
/// from `pending_recent`.
fn store_recent(
    tx: &Transaction<'_>,
    batch: &RecentBatch,
    desktop: &[DesktopUri],
    pending_recent: &mut HashSet<i64>,
) -> Result<(), ContentFailure> {
    let mut rows: Vec<(RecentRow, bool)> = Vec::new();
    for (entry, desktop_uri) in batch.entries.iter().zip(desktop) {
        // An entry whose package has no stored app is skipped and stays pending.
        let Some(app) = store::app_in(tx, &entry.package_name)? else { continue };
        let desktop_uri = match desktop_uri {
            DesktopUri::None => None,
            DesktopUri::Kept(uri) => Some(uri.clone()),
            DesktopUri::NotAbsolute => {
                return Err(ContentFailure::Refused("a recent entry's desktop URI is not an absolute URI"));
            }
        };

        pending_recent.remove(&entry.id);
        let name = if entry.description.is_empty() { app.name } else { entry.description.clone() };
        rows.push((
            RecentRow {
                recent_id: entry.id,
                package_name: entry.package_name.clone(),
                name,
                checksum: Some(entry.checksum),
                rank: entry.rank,
                intent_action: entry.intent_action.clone(),
                task_id: entry.task_id,
                web_uri: entry.web_uri.clone(),
                desktop_uri,
            },
            entry.delete,
        ));
    }

    if batch.with_actions {
        // The phone sends every current entry: the message replaces the list.
        store::clear_recent(tx)?;
        for (row, _) in rows.iter().filter(|(_, delete)| !delete) {
            store::upsert_recent(tx, row)?;
        }
    } else {
        for (row, _) in &rows {
            store::upsert_recent(tx, row)?;
        }
    }
    Ok(())
}

/// The store failed while applying a message: a constraint (an id another package holds) is the
/// phone's data, anything else the store's.
fn store_rejection(e: &rusqlite::Error) -> Rejection {
    if matches!(e.sqlite_error_code(), Some(rusqlite::ErrorCode::ConstraintViolation)) {
        return rejection("an item's id is held by another package");
    }
    rejection("phone apps store failure").because(e)
}

/// Why an item message's transaction was rolled back.
enum ContentFailure {
    Store(rusqlite::Error),
    Refused(&'static str),
}

impl From<rusqlite::Error> for ContentFailure {
    fn from(e: rusqlite::Error) -> Self {
        ContentFailure::Store(e)
    }
}

/// What a recent entry's desktop URI comes to before it is stored.
enum DesktopUri {
    /// None received, dropped by the URI rule, or no installed application opens it.
    None,
    Kept(String),
    /// Not an absolute URI: the message is refused when the entry's package has a stored app.
    NotAbsolute,
}

/// The factory of `phoneApps` processors.
pub(crate) struct Factory {
    pub(crate) inner: Arc<Inner>,
}

impl ContentProcessorFactory for Factory {
    fn create(
        &self,
        ctx: &InboundContext<'_>,
        payload: &ValueSet,
    ) -> Result<Option<Box<dyn ContentProcessor>>, Rejection> {
        // An item message with no open sync is processed on its own; it needs a correlation vector.
        if !payload.contains_key(app_key::METADATA) && ctx.correlation_vector(payload).is_none() {
            return Err(rejection("item message without a correlationVector"));
        }
        Ok(Some(Box::new(Processor::new(self.inner.clone()))))
    }
}

/// One sync (or one item message processed on its own).
pub(crate) struct Processor {
    inner: Arc<Inner>,
    pending_apps: HashSet<i64>,
    pending_recent: HashSet<i64>,
    /// `-1`: none received in this sync.
    apps_sequence: i64,
    recent_sequence: i64,
    /// The favorite ranks stored when the metadata message came, by package.
    favorites: HashMap<String, i64>,
}

/// Writes the sync's completion: the app list's sync time once no phone-app id is pending, the
/// sequence numbers received once either pending set is empty.
fn complete(
    tx: &Transaction<'_>,
    now_ms: i64,
    apps_done: bool,
    either_done: bool,
    apps_sequence: i64,
    recent_sequence: i64,
) -> rusqlite::Result<()> {
    if apps_done {
        store::record_sync_time(tx, now_ms)?;
    }

    if either_done {
        if apps_sequence != -1 {
            store::set_current(tx, Content::PhoneApps, apps_sequence)?;
        }
        if recent_sequence != -1 {
            store::set_current(tx, Content::RecentApps, recent_sequence)?;
        }
    }
    Ok(())
}

impl Processor {
    fn new(inner: Arc<Inner>) -> Self {
        Self {
            inner,
            pending_apps: HashSet::new(),
            pending_recent: HashSet::new(),
            apps_sequence: -1,
            recent_sequence: -1,
            favorites: HashMap::new(),
        }
    }

    fn state(&self, ctx: &InboundContext<'_>) -> Result<Arc<PhoneState>, Rejection> {
        self.inner.phone_state(ctx.phone).map_err(|e| {
            error!(phone = %ctx.phone, error = %e, "phone apps store not opened; the message is rejected");
            rejection("phone apps store unavailable")
        })
    }

    /// The remoting capabilities the phone record holds.
    fn stored_flags(ctx: &InboundContext<'_>) -> u32 {
        ctx.app.phone_record().filter(|r| r.phone() == *ctx.phone).map_or(0, |r| r.remoting_flags())
    }

    /// Writes the remoting capabilities a message reported, once it is applied.
    fn apply_settings(ctx: &InboundContext<'_>, settings: Option<u32>) {
        if let Some(flags) = settings {
            ctx.app.update_phone_record(|r| r.set_remoting_flags(flags));
        }
    }

    /// The desktop URI of each recent entry: the URI rule, then it must be an absolute URI, then an
    /// installed application must open it.
    fn desktop_uris(&self, items: &Items) -> Vec<DesktopUri> {
        let Some(recent) = &items.recent else { return Vec::new() };
        recent
            .entries
            .iter()
            .map(|e| {
                let Some(candidate) = e.desktop_uri.as_deref().and_then(|uri| {
                    wire::desktop_uri_candidate(&e.package_name, uri, self.inner.config.office_desktop_uri_rewrite)
                }) else {
                    return DesktopUri::None;
                };
                if !wire::is_absolute_uri(&candidate) {
                    return DesktopUri::NotAbsolute;
                }
                if self.inner.host.can_open_uri(candidate.clone()) {
                    DesktopUri::Kept(candidate)
                } else {
                    DesktopUri::None
                }
            })
            .collect()
    }
}

#[async_trait]
impl ContentProcessor for Processor {
    async fn metadata(&mut self, ctx: &InboundContext<'_>, payload: &ValueSet) -> Processed {
        let m = match wire::parse_metadata(payload, Self::stored_flags(ctx)) {
            Ok(m) => m,
            Err(m) => return Processed::Rejected(rejection(m.reason)),
        };

        let ps = match self.state(ctx) {
            Ok(ps) => ps,
            Err(r) => return Processed::Rejected(r),
        };

        let apps_sequence = m.apps_sequence.unwrap_or(-1);
        let recent_sequence = m.recent_sequence.unwrap_or(-1);
        let now = self.inner.now_ms();
        let applied = ps.db.transaction(|tx| {
            let favorites = store::favorite_ranks(tx)?;
            let apps = m.apps.as_ref().map(|l| store::synchronize_apps(tx, l)).transpose()?.unwrap_or_default();
            let recent = m.recent.as_ref().map(|l| store::synchronize_recent(tx, l)).transpose()?.unwrap_or_default();
            if apps.requested.is_empty() && recent.requested.is_empty() {
                complete(tx, now, true, true, apps_sequence, recent_sequence)?;
            }
            Ok::<_, rusqlite::Error>((favorites, apps, recent))
        });
        let (favorites, apps, recent) = match applied {
            Ok(a) => a,
            Err(e) => return Processed::Rejected(store_rejection(&e)),
        };

        Self::apply_settings(ctx, m.settings);
        self.favorites = favorites;
        self.pending_apps = apps.requested.iter().copied().collect();
        self.pending_recent = recent.requested.iter().copied().collect();
        self.apps_sequence = apps_sequence;
        self.recent_sequence = recent_sequence;

        if apps.deleted > 0 {
            self.inner.emit(PhoneAppsEvent::AppsChanged { phone: ctx.phone.clone() });
        }
        if recent.deleted > 0 {
            self.inner.emit(PhoneAppsEvent::RecentChanged { phone: ctx.phone.clone() });
        }

        let answer = wire::metadata_reply(
            &m.correlation_vector,
            ctx.app.standard_keys().contract_version,
            &apps.requested,
            m.recent_supported.then_some(recent.requested.as_slice()),
        );

        if apps.requested.is_empty() && recent.requested.is_empty() {
            info!(phone = %ctx.phone, "phoneApps sync finished; nothing requested");
            self.inner.refresh_sync_state(&ps);
            Processed::completed(answer)
        } else {
            info!(
                phone = %ctx.phone,
                app_count = apps.requested.len(),
                recent_count = recent.requested.len(),
                "phoneApps sync started"
            );
            Processed::open(answer)
        }
    }

    async fn content(&mut self, ctx: &InboundContext<'_>, payload: &ValueSet) -> Processed {
        let items = match wire::parse_items(payload, Self::stored_flags(ctx)) {
            Ok(i) => i,
            Err(m) => return Processed::Rejected(rejection(m.reason)),
        };

        let ps = match self.state(ctx) {
            Ok(ps) => ps,
            Err(r) => return Processed::Rejected(r),
        };

        // Checked before the transaction: the edge may block.
        let desktop = self.desktop_uris(&items);
        let mut pending_apps = self.pending_apps.clone();
        let mut apps_sequence = self.apps_sequence;
        if let Some(batch) = &items.apps {
            for item in &batch.items {
                pending_apps.remove(&item.id);
            }
            if let Some(s) = batch.sequence {
                apps_sequence = s;
            }
        }

        let mut recent_sequence = self.recent_sequence;
        if let Some(s) = items.recent.as_ref().and_then(|r| r.sequence) {
            recent_sequence = s;
        }

        let now = self.inner.now_ms();
        let favorites = &self.favorites;
        let pending_recent_before = &self.pending_recent;
        let applied = ps.db.transaction(|tx| {
            if let Some(batch) = &items.apps {
                store_apps(tx, batch, favorites)?;
            }
            let mut pending_recent = pending_recent_before.clone();
            if let Some(batch) = &items.recent {
                store_recent(tx, batch, &desktop, &mut pending_recent)?;
            }
            let apps_done = pending_apps.is_empty();
            complete(tx, now, apps_done, apps_done || pending_recent.is_empty(), apps_sequence, recent_sequence)?;
            Ok::<_, ContentFailure>(pending_recent)
        });
        let pending_recent = match applied {
            Ok(p) => p,
            Err(ContentFailure::Store(e)) => return Processed::Rejected(store_rejection(&e)),
            Err(ContentFailure::Refused(reason)) => return Processed::Rejected(rejection(reason)),
        };

        Self::apply_settings(ctx, items.settings);
        if items.apps.is_none() && items.recent.is_none() {
            warn!(phone = %ctx.phone, "phoneApps message with neither phone-app nor recent items; applied as empty");
        }

        self.pending_apps = pending_apps;
        self.pending_recent = pending_recent;
        self.apps_sequence = apps_sequence;
        self.recent_sequence = recent_sequence;

        if items.apps.is_some() {
            self.inner.emit(PhoneAppsEvent::AppsChanged { phone: ctx.phone.clone() });
        }
        if items.recent.is_some() {
            self.inner.emit(PhoneAppsEvent::RecentChanged { phone: ctx.phone.clone() });
        }

        self.inner.refresh_sync_state(&ps);

        let completed = self.pending_apps.is_empty();
        if completed {
            info!(phone = %ctx.phone, "phoneApps sync finished");
        }
        Processed::Answer { answer: simple_answer(0), completed }
    }
}
