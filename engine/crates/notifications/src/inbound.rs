//! Inbound notification batches (phone → PC): the `notifications` content processor's metadata
//! phase (the batch announcement) and content phase (the content message, applying the batch, the
//! reply), and the presentation that follows an applied batch.
//!
//! A content message is validated whole before anything changes: the three arrays (exact types,
//! equal lengths — else the empty answer, transaction discarded), then duplicate item JSON
//! among the upserts (rejected `result 1` + `message`). Malformed items are skipped. The batch's
//! store changes are one transaction; a failing transaction is rejected like a duplicate.

use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use tracing::{debug, error, warn};

use phonelink_app::consts::{key as app_key, result_code};
use phonelink_app::{
    AppLayer, ContentProcessor, ContentProcessorFactory, InboundContext, Processed, Rejection, rejection_message,
    simple_answer,
};
use phonelink_core::codec::valueset::{Value, ValueSet};
use phonelink_core::types::DcgClient;

use super::item::NotificationItem;
use super::presentation::{self, DecisionInput, ThrottleDecision};
use super::settings::AppFlags;
use super::store::{self, BatchEffects, BatchPlan};
use super::wire::{CONTENT_TYPE, key, operation};
use super::{Inner, NotificationsEvent, app_list_supported, device_permission, value_text};

const ARRAYS_MISSING: &str = "notification arrays missing or mistyped";
const ARRAYS_UNEQUAL: &str = "notification arrays of unequal length";
const DUPLICATE_ITEM: &str = "duplicate notification item";
const STORE_FAILURE: &str = "notification store failure";

/// A rejection answered `{ result: Int32 1, message: String "<reason> 80004005" }`.
fn failure(reason: &'static str) -> Rejection {
    Rejection::with_answer(
        reason,
        ValueSet::new().with(app_key::RESULT, result_code::FAILURE).with(key::MESSAGE, rejection_message(reason)),
    )
}

fn is_upsert(op: i32) -> bool {
    op == operation::NEW_NOTIFICATION || op == operation::EXISTING_NOTIFICATION
}

/// Creates one processor per transaction from the message that opens it.
pub(crate) struct BatchFactory {
    inner: Arc<Inner>,
}

impl BatchFactory {
    pub(crate) fn new(inner: &Arc<Inner>) -> Self {
        Self { inner: inner.clone() }
    }
}

impl ContentProcessorFactory for BatchFactory {
    /// The processor keeps the message's correlation id (the text form of `correlationVector`) and
    /// its expected operation count: the length of `notificationKeys` when it is a `StringArray`,
    /// `0` otherwise and whenever the correlation id is missing.
    fn create(
        &self,
        _ctx: &InboundContext<'_>,
        payload: &ValueSet,
    ) -> Result<Option<Box<dyn ContentProcessor>>, Rejection> {
        let correlation_id = payload.get(app_key::CORRELATION_VECTOR).and_then(value_text);
        let expected = match (&correlation_id, payload.string_array(key::NOTIFICATION_KEYS)) {
            (Some(_), Some(keys)) => keys.len(),
            (Some(_), None) => 0,
            (None, _) => {
                warn!("notifications message without a correlationVector; its batch never completes");
                0
            }
        };
        Ok(Some(Box::new(BatchProcessor { inner: self.inner.clone(), correlation_id, expected })))
    }
}

struct BatchProcessor {
    inner: Arc<Inner>,
    correlation_id: Option<String>,
    expected: usize,
}

#[async_trait]
impl ContentProcessor for BatchProcessor {
    /// The batch announcement: `correlationVector` (an `Empty` value when the message has none),
    /// `contentType`, `contractVersion`, `itemCount`, `result 0`; complete when the count is `0`.
    async fn metadata(&mut self, ctx: &InboundContext<'_>, _payload: &ValueSet) -> Processed {
        let correlation = match &self.correlation_id {
            Some(c) => Value::String(c.clone()),
            None => Value::Empty,
        };

        let count = i32::try_from(self.expected).unwrap_or(i32::MAX);
        let answer = ValueSet::new()
            .with(app_key::CORRELATION_VECTOR, correlation)
            .with(app_key::CONTENT_TYPE, CONTENT_TYPE)
            .with(app_key::CONTRACT_VERSION, ctx.app.standard_keys().contract_version)
            .with(key::ITEM_COUNT, count)
            .with(app_key::RESULT, result_code::SUCCESS);
        Processed::Answer { answer, completed: self.expected == 0 }
    }

    async fn content(&mut self, ctx: &InboundContext<'_>, payload: &ValueSet) -> Processed {
        self.inner.process_batch(ctx, payload, self.expected)
    }
}

impl Inner {
    /// One content message: validate, apply in one transaction, present, answer `result 0`. The
    /// batch completes — "last updated = now" — when this message carries exactly the expected
    /// number of operations.
    fn process_batch(&self, ctx: &InboundContext<'_>, payload: &ValueSet, expected: usize) -> Processed {
        let phone = ctx.phone;
        let (Some(keys), Some(ops), Some(texts)) = (
            payload.string_array(key::NOTIFICATION_KEYS),
            payload.i32_array(key::OPERATIONS),
            payload.string_array(key::NOTIFICATIONS),
        ) else {
            return Processed::Rejected(Rejection::with_answer(ARRAYS_MISSING, ValueSet::new()));
        };

        if keys.len() != ops.len() || ops.len() != texts.len() {
            return Processed::Rejected(Rejection::with_answer(ARRAYS_UNEQUAL, ValueSet::new()));
        }

        let mut seen: HashSet<&str> = HashSet::new();
        for (op, text) in ops.iter().zip(texts) {
            if is_upsert(*op) && !seen.insert(text.as_str()) {
                return Processed::Rejected(failure(DUPLICATE_ITEM));
            }
        }

        let mut items: Vec<(NotificationItem, &str, i32)> = Vec::new();
        let mut malformed = (0usize, None);
        for (position, (op, text)) in ops.iter().zip(texts).enumerate() {
            if !is_upsert(*op) {
                continue;
            }
            match NotificationItem::parse(text) {
                Ok(item) => items.push((item, text.as_str(), *op)),
                Err(e) => malformed = (malformed.0 + 1, malformed.1.or(Some((position, e.reason())))),
            }
        }

        if let (skipped_count, Some((position, reason))) = malformed {
            warn!(skipped_count, position, reason, "malformed notification items skipped");
        }

        let mut plan = BatchPlan::default();
        for (op, k) in ops.iter().zip(keys) {
            match *op {
                operation::SESSION_CHANGE if !k.is_empty() => plan.session_changes.push(k.as_str()),
                operation::REMOVE_NOTIFICATION => plan.removals.push(k.as_str()),
                _ => {}
            }
        }
        plan.upserts = items.iter().map(|(item, text, op)| (item, *text, *op)).collect();

        let effects = match self.db(phone).and_then(|db| store::apply_batch(&db, &plan)) {
            Ok(effects) => effects,
            Err(e) => {
                return Processed::Rejected(failure(STORE_FAILURE).because(e));
            }
        };
        self.after_batch(ctx.app, phone, &items, &effects);

        let completed = ops.len() == expected;
        if completed {
            self.set_last_updated(phone);
        }
        Processed::Answer { answer: simple_answer(result_code::SUCCESS), completed }
    }

    /// Withdraws what the batch deleted and presents its new items: first the session changes'
    /// rows, then the alerts for operation-1 items whose key is still stored, then the removed rows.
    fn after_batch(
        &self,
        app: &AppLayer,
        phone: &DcgClient,
        items: &[(NotificationItem, &str, i32)],
        effects: &BatchEffects,
    ) {
        self.withdraw(phone, effects.purged.iter().map(|r| r.key.clone()));

        let removed: HashSet<&str> = effects.removed.iter().map(|r| r.key.as_str()).collect();
        let candidates: Vec<(&NotificationItem, bool)> = items
            .iter()
            .zip(&effects.upserted)
            .filter(|((item, _, op), _)| *op == operation::NEW_NOTIFICATION && !removed.contains(item.key.as_str()))
            .map(|((item, _, _), up)| (item, up.is_new))
            .collect();

        if !candidates.is_empty() {
            if let Err(e) = self.present(app, phone, &candidates) {
                error!(error = %e, "notifications store not read; the batch's banners are not shown");
            }
        }

        self.withdraw(phone, effects.removed.iter().map(|r| r.key.clone()));
        if !effects.purged.is_empty() || !effects.upserted.is_empty() || !effects.removed.is_empty() {
            self.emit(NotificationsEvent::ListChanged { phone: phone.clone() });
        }
    }

    /// The banner decision and posting for each candidate.
    fn present(
        &self,
        app: &AppLayer,
        phone: &DcgClient,
        candidates: &[(&NotificationItem, bool)],
    ) -> Result<(), phonelink_features::StoreError> {
        let Some(record) = app.phone_record().filter(|r| r.phone().is_same_client(phone)) else {
            return Ok(());
        };

        let db = self.db(phone)?;
        let pc_permission = Self::pc_permission(&db)?;
        let per_app = app_list_supported(&record);
        let muted_all = self.muted_all(&db)?;
        let throttling = self.config.toast_throttling && Self::throttling_enabled(&db)?;

        let key_window = app.is_foreground();
        let now = self.clock.now();
        let now_ms = self.now_ms();
        let now_ticks = phonelink_core::time::ticks_from_unix_ms(now_ms);
        let throttle_cfg = super::throttle_config(&self.config);

        for (item, is_new) in candidates {
            let (flags, mute_ticks) =
                if per_app { self.app_flags(&db, &item.package_name)? } else { (AppFlags::ALL, 0) };
            let duplicate = self.with_phone(phone, |p| p.dedupe.check(item, now, self.config.dedupe_window));
            let group = item.is_read_only_group_message();
            let input = DecisionInput {
                is_new: *is_new,
                duplicate,
                pc_permission,
                device_permission: device_permission(&record),
                group_alerts_allowed: group
                    && self.messaging.as_ref().is_some_and(|m| m.group_message_alerts_allowed(phone)),
                muted_all,
                app_muted: mute_ticks > now_ticks,
                app_flags: flags,
                key_window,
                now_ms,
                recency_ms: i64::try_from(self.config.banner_recency.as_millis()).unwrap_or(i64::MAX),
            };

            if let Err(reason) = presentation::decide(item, &input) {
                debug!(?reason, "phone notification not presented");
                continue;
            }

            let popup = presentation::popup_allowed(item, flags);
            let (alert, notice_alert) = if popup && throttling {
                match self.with_phone(phone, |p| p.throttle.process(&item.package_name, now, &throttle_cfg)) {
                    ThrottleDecision::ShowNormally => (true, None),
                    ThrottleDecision::ShowSilently => (group, None),
                    ThrottleDecision::ShowSilentlyWithNotice => (group, Some(!group)),
                }
            } else {
                (popup, None)
            };

            let mut banner = presentation::banner(phone, item, alert);
            // The app's icon from the phone's app list; none (the app's own icon) when it is unknown.
            banner.image = self.phone_app(phone, &item.package_name).and_then(|a| a.icon);
            self.post(banner);

            if let Some(notice_alert) = notice_alert {
                match item.app_name().filter(|n| !n.is_empty()) {
                    Some(app_name) if !item.package_name.is_empty() => self.post(presentation::paused_notice(
                        phone,
                        app_name,
                        &item.package_name,
                        self.config.throttling_cooldown,
                        notice_alert,
                    )),
                    _ => warn!("paused notice not posted; the throttled app has no name"),
                }
            }
        }
        Ok(())
    }
}
