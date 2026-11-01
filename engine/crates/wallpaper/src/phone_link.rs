//! Phone Link's contract: the `wallpaper` content processor and connect provider #4.
//!
//! Request: `configuration.Wallpaper` (`Bool`) = the phone is supported (its contract version is
//! at least `3.84`); `sequenceNumbers.wallpaper` (`Int64`, the stored *current*, `-1` when none) when
//! it is also capable (capability bit `0x200`) — left out when the store cannot be read. Reply, read
//! from the top level: with the gates **as they stood when the request was built** (the provider runs
//! before the reply's contract version and capability bits are recorded) and `result` `Int32 0`, the
//! *expected* sequence number = `wallpaper` (`Int64`, else `-1`); whatever `result` is,
//! `permissions.read_media_images` / `permissions.files` (`Bool`) set those device permissions and
//! `wallpaperDevicePermission` (`Bool`) the phone's wallpaper permission. A mistyped key is skipped.

use std::collections::VecDeque;
use std::sync::{Mutex, Weak};

use async_trait::async_trait;
use tracing::{error, info, warn};

use phonelink_app::consts::{capability, key as app_key, permission, result_code};
use phonelink_app::{
    ConnectContext, ConnectContribution, ConnectContributor, ContentProcessor, ContentProcessorFactory, InboundContext,
    PhoneRecord, Processed, Rejection, simple_answer,
};
use phonelink_core::codec::valueset::{Value, ValueSet};
use phonelink_core::config::AppConfig;

use super::Inner;
use super::store::{self, HOME_SCREEN};
use phonelink_core::sync::MutexExt;

/// Why the stored wallpaper state could not be read for a connect.
#[derive(Debug, thiserror::Error)]
enum StateUnreadable {
    #[error("wallpaper feature stopped")]
    Stopped,
    #[error("{0}")]
    Feature(#[source] phonelink_features::FeatureError),
    #[error("{0}")]
    Store(#[source] phonelink_features::StoreError),
}

/// The `contentType` of wallpaper content.
pub const CONTENT_TYPE: &str = "wallpaper";
/// The lowest contract version the phone's wallpaper is supported from.
pub const MIN_CONTRACT_VERSION: f64 = 3.84;

/// The keys of this contract.
pub mod key {
    pub const CONFIGURATION_WALLPAPER: &str = "Wallpaper";
    /// The sequence-number key, in the request's `sequenceNumbers` and the reply's top level.
    pub const SEQUENCE: &str = "wallpaper";
    pub const WALLPAPER_ID: &str = "wallpaperId";
    pub const WALLPAPER_TYPE: &str = "wallpaperType";
    pub const WALLPAPER: &str = "wallpaper";
    pub const SEQUENCE_NUMBER: &str = "sequenceNumber";
    pub const PERMISSIONS: &str = "permissions";
    pub const READ_MEDIA_IMAGES: &str = "read_media_images";
    pub const FILES: &str = "files";
    pub const DEVICE_PERMISSION: &str = "wallpaperDevicePermission";
}

/// The phone's wallpaper is supported: its contract version is at least `3.84`.
pub fn supported(record: &PhoneRecord, config: &AppConfig) -> bool {
    record.contract_version(config) >= MIN_CONTRACT_VERSION
}

/// Supported, and the phone has the `Wallpaper` capability bit.
pub fn capable_and_supported(record: &PhoneRecord, config: &AppConfig) -> bool {
    record.has_capability(capability::WALLPAPER) && supported(record, config)
}

// ------------------------------------------------------------------------------------------------
// wallpaper content
// ------------------------------------------------------------------------------------------------

/// Creates the `wallpaper` processor. A message without `correlationVector` (the dispatcher has
/// copied the header's in when the payload lacks it) is rejected.
pub(crate) struct WallpaperFactory(pub(crate) Weak<Inner>);

impl ContentProcessorFactory for WallpaperFactory {
    fn create(
        &self,
        _ctx: &InboundContext<'_>,
        payload: &ValueSet,
    ) -> Result<Option<Box<dyn ContentProcessor>>, Rejection> {
        if !payload.contains_key(app_key::CORRELATION_VECTOR) {
            return Err(Rejection::new("wallpaper without correlationVector"));
        }
        Ok(Some(Box::new(WallpaperProcessor(self.0.clone()))))
    }
}

struct WallpaperProcessor(Weak<Inner>);

/// A content item with all four keys, typed as Phone Link reads them.
struct Item<'a> {
    id: i64,
    wallpaper_type: i64,
    bytes: &'a [u8],
    sequence: i64,
}

/// `None`: one of the four keys is absent (nothing is stored). `Err`: all four are present and one
/// has another type.
fn read_item(payload: &ValueSet) -> Result<Option<Item<'_>>, Rejection> {
    let (Some(id), Some(wallpaper_type), Some(bytes), Some(sequence)) = (
        payload.get(key::WALLPAPER_ID),
        payload.get(key::WALLPAPER_TYPE),
        payload.get(key::WALLPAPER),
        payload.get(key::SEQUENCE_NUMBER),
    ) else {
        return Ok(None);
    };

    match (id, wallpaper_type, bytes, sequence) {
        (Value::Int64(id), Value::Int64(wallpaper_type), Value::UInt8Array(bytes), Value::Int64(sequence)) => {
            Ok(Some(Item { id: *id, wallpaper_type: *wallpaper_type, bytes, sequence: *sequence }))
        }
        _ => Err(Rejection::new("wallpaper item key of another type")),
    }
}

#[async_trait]
impl ContentProcessor for WallpaperProcessor {
    /// The metadata step: an empty answer (no `result`), the transaction open.
    async fn metadata(&mut self, _ctx: &InboundContext<'_>, _payload: &ValueSet) -> Processed {
        Processed::open(ValueSet::new())
    }

    async fn content(&mut self, ctx: &InboundContext<'_>, payload: &ValueSet) -> Processed {
        let item = match read_item(payload) {
            Ok(item) => item,
            Err(rejection) => return Processed::Rejected(rejection),
        };
        let Some(item) = item else {
            warn!(phone = %ctx.phone, "wallpaper item without all four keys; nothing stored");
            return Processed::completed(simple_answer(result_code::SUCCESS));
        };
        let Some(inner) = self.0.upgrade() else {
            return Processed::Rejected(Rejection::new("wallpaper feature stopped"));
        };

        let stored = inner.db(ctx.phone).and_then(|db| {
            Ok(store::put_phone_link_item(&db, item.wallpaper_type, item.id, item.bytes, item.sequence)?)
        });
        if let Err(e) = stored {
            return Processed::Rejected(Rejection::new("wallpaper store failure").because(e));
        }

        info!(
            phone = %ctx.phone,
            wallpaper_type = item.wallpaper_type,
            bytes = item.bytes.len(),
            sequence = item.sequence,
            "phone wallpaper stored"
        );

        if item.wallpaper_type == HOME_SCREEN {
            inner.changed(ctx.phone);
        }
        Processed::completed(simple_answer(result_code::SUCCESS))
    }
}

// ------------------------------------------------------------------------------------------------
// connect provider #4
// ------------------------------------------------------------------------------------------------

/// How many built connects' gates are remembered until their reply is applied.
const REMEMBERED_GATES: usize = 16;

/// The provider registered for [`phonelink_app::ConnectProvider::Wallpaper`].
pub(crate) struct WallpaperContributor {
    pub(crate) inner: Weak<Inner>,
    /// `(correlationVector, capable and supported)` of the connects built and not yet applied.
    gates: Mutex<VecDeque<(String, bool)>>,
}

impl WallpaperContributor {
    pub(crate) fn new(inner: Weak<Inner>) -> Self {
        Self { inner, gates: Mutex::new(VecDeque::new()) }
    }

    fn remember(&self, correlation_vector: &str, gate: bool) {
        let mut gates = self.gates.lock_or_recover();
        if gates.len() == REMEMBERED_GATES {
            gates.pop_front();
        }
        gates.push_back((correlation_vector.to_owned(), gate));
    }

    fn take(&self, correlation_vector: &str) -> Option<bool> {
        let mut gates = self.gates.lock_or_recover();
        let i = gates.iter().position(|(cv, _)| cv == correlation_vector)?;
        gates.remove(i).map(|(_, gate)| gate)
    }
}

#[async_trait]
impl ConnectContributor for WallpaperContributor {
    async fn contribute(&self, ctx: &ConnectContext<'_>) -> ConnectContribution {
        let config = &ctx.app.config().app;
        let mut c = ConnectContribution::default();
        c.configuration.insert(key::CONFIGURATION_WALLPAPER, supported(ctx.record, config));

        let gate = capable_and_supported(ctx.record, config);
        self.remember(ctx.correlation_vector, gate);

        if gate {
            let current = self
                .inner
                .upgrade()
                .ok_or(StateUnreadable::Stopped)
                .and_then(|inner| inner.db(ctx.phone).map_err(StateUnreadable::Feature))
                .and_then(|db| store::phone_link_state(&db).map_err(StateUnreadable::Store));
            match current {
                Ok(state) => c.sequence_numbers.insert(key::SEQUENCE, Value::Int64(state.current_sequence)),
                // An unreadable store is not "nothing stored": no value is stated.
                Err(e) => {
                    warn!(phone = %ctx.phone, error = %e, "wallpaper store unreadable; sequence number left out of the connect")
                }
            }
        }

        c
    }

    async fn apply(&self, ctx: &ConnectContext<'_>, response: &ValueSet) {
        let gate = match self.take(ctx.correlation_vector) {
            Some(gate) => gate,
            None => {
                warn!(phone = %ctx.phone, "no wallpaper gate remembered for this connect; using the record after the reply");
                capable_and_supported(ctx.record, &ctx.app.config().app)
            }
        };

        let Some(inner) = self.inner.upgrade() else { return };
        let db = match inner.db(ctx.phone) {
            Ok(db) => Some(db),
            Err(e) => {
                error!(phone = %ctx.phone, error = %e, "wallpaper store unavailable; its connect keys not stored");
                None
            }
        };

        if gate && response.get(app_key::RESULT) == Some(&Value::Int32(0)) {
            let expected = response.i64(key::SEQUENCE).unwrap_or(-1);
            if let Some(db) = &db {
                if let Err(e) = store::set_expected_sequence(db, expected) {
                    error!(phone = %ctx.phone, error = %e, "wallpaper expected sequence number not stored");
                }
            }
        }

        if let Some(permissions) = response.object(key::PERMISSIONS) {
            for (name, bit) in
                [(key::READ_MEDIA_IMAGES, permission::READ_MEDIA_IMAGES), (key::FILES, permission::FILES)]
            {
                if let Some(granted) = permissions.bool(name) {
                    ctx.app.update_record_for(ctx.phone, |r| r.set_permission(bit, granted));
                }
            }
        }

        if let (Some(granted), Some(db)) = (response.bool(key::DEVICE_PERMISSION), &db) {
            match store::set_device_permission(db, granted) {
                Ok(changed) => {
                    if changed {
                        info!(phone = %ctx.phone, granted, "phone wallpaper permission changed");
                    }
                }
                Err(e) => error!(phone = %ctx.phone, error = %e, "wallpaper permission not stored"),
            }
        }

        // The shown wallpaper may follow the reply's contract version and permission.
        inner.changed(ctx.phone);
    }
}
