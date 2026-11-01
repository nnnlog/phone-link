//! `/nanohandshake`: how screen mirroring exchanges a Nano connection attempt's offer and answer with
//! the phone — a KVP App request carrying the offer with the mirroring keys, answered with the
//! phone's handshake.

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::SystemTime;

use async_trait::async_trait;
use serde_json::{Map, Value, json};
use tracing::debug;

use phonelink_app::{AppLayer, AppOutcome, AppRequest, RequestOptions};
use phonelink_core::codec::valueset::ValueSet;
use phonelink_core::config::NanoConfig;
use phonelink_nano::connect::{Attempt, AttemptError, Exchange, INSTANCE_ID};

/// The handshake contract version range both sides use.
const HANDSHAKE_CONTRACT_VERSION: u64 = 6;

pub(super) const NANOHANDSHAKE_ROUTE: &str = "/nanohandshake";

/// The last `sequenceNumber` sent, so every handshake's is greater.
static LAST_SEQUENCE: AtomicI64 = AtomicI64::new(0);

/// The current UTC time as a Windows FILETIME (100 ns since 1601-01-01), increasing per call.
pub(super) fn next_sequence_number() -> i64 {
    let now = phonelink_core::time::filetime(SystemTime::now());
    let mut last = LAST_SEQUENCE.load(Ordering::Relaxed);
    loop {
        let next = now.max(last + 1);
        match LAST_SEQUENCE.compare_exchange(last, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return next,
            Err(actual) => last = actual,
        }
    }
}

/// `/nanohandshake`: the offer, with the mirroring keys added, as a KVP App request.
pub(super) struct NanoHandshake {
    pub app: Arc<AppLayer>,
    /// The workflow's correlation id (the phone requires it).
    pub correlation_vector: String,
    pub workflow: String,
    /// The PC supports and enabled audio.
    pub audio: bool,
    /// Sent with the first attempt when set (the phone's NAT test).
    pub diagnostics_servers: Option<String>,
}

impl NanoHandshake {
    /// The offer with the top-level keys of the screen-mirroring path.
    pub(super) fn offer(&self, attempt: &Attempt, mut offer: Map<String, Value>) -> Map<String, Value> {
        offer.insert(
            "contractVersion".into(),
            json!({"versionMin": HANDSHAKE_CONTRACT_VERSION, "versionMax": HANDSHAKE_CONTRACT_VERSION}),
        );
        offer.insert(
            "settings".into(),
            json!({"remoteTaskTransfer": true, "connectionType": attempt.transport, "audio": self.audio}),
        );
        offer.insert("instanceId".into(), Value::String(INSTANCE_ID.clone()));
        offer.insert("multipleConnections".into(), Value::Bool(attempt.multiple_connections));

        if attempt.first {
            if let Some(d) = &self.diagnostics_servers {
                offer.insert("diagnosticsServers".into(), Value::String(d.clone()));
            }
        }
        offer
    }

    /// The comma-joined diagnostics hosts, when the configuration names at least two.
    pub(super) fn diagnostics_from(config: &NanoConfig) -> Option<String> {
        let hosts: Vec<&str> =
            config.mirroring_diagnostics_servers.split(',').map(str::trim).filter(|h| !h.is_empty()).collect();
        (hosts.len() >= 2).then(|| hosts.join(","))
    }
}

/// The send status of a `/nanohandshake` that got no reply: `603` timed out, `608` no network, `606`
/// the phone unreachable (no connection, not delivered, cancelled), else `609`.
pub(super) fn handshake_send_status(outcome: &AppOutcome) -> i32 {
    match outcome {
        AppOutcome::TimedOut => 603,
        AppOutcome::InternetError => 608,
        AppOutcome::ConnectionFailed(_) | AppOutcome::SendFailed(_) | AppOutcome::Canceled => 606,
        _ => 609,
    }
}

/// The phone's handshake contract version: `min(versionMax, 6)`, 0 when absent.
pub(super) fn answer_contract_version(answer: &Value) -> u64 {
    answer
        .get("contractVersion")
        .and_then(|c| c.get("versionMax"))
        .and_then(Value::as_u64)
        .map_or(0, |v| v.min(HANDSHAKE_CONTRACT_VERSION))
}

#[async_trait]
impl Exchange for NanoHandshake {
    async fn exchange(&self, attempt: &Attempt, offer: Map<String, Value>) -> Result<Value, AttemptError> {
        let handshake = Value::Object(self.offer(attempt, offer)).to_string();

        let mut values = ValueSet::new();
        values.insert("isuserinitiated", true);
        values.insert("correlationVector", self.correlation_vector.clone());
        values.insert("contentType", "nanoHandshake");
        values.insert("handshake", handshake);
        values.insert("sequenceNumber", next_sequence_number());
        values.insert("workflow", self.workflow.clone());

        let request = AppRequest::kvp(NANOHANDSHAKE_ROUTE, values).with_options(RequestOptions::user_initiated());
        let response = self.app.request(request).await.map_err(|e| AttemptError::Exchange(e.to_string()))?;

        let Some(values) = response.values() else {
            return Err(AttemptError::Transport { status: handshake_send_status(&response.outcome) });
        };
        let result = response.result.unwrap_or(-1);
        if result != 0 {
            return Err(AttemptError::Rejected { result, reason: values.str("rejectReason").map(str::to_string) });
        }

        let text = values.str("handshake").ok_or_else(|| AttemptError::BadAnswer("no handshake".into()))?;
        let answer: Value = serde_json::from_str(text).map_err(|e| AttemptError::BadAnswer(e.to_string()))?;
        if !answer.is_object() {
            return Err(AttemptError::BadAnswer("the handshake is not an object".into()));
        }

        debug!(contract = answer_contract_version(&answer), "nanohandshake answered");
        Ok(answer)
    }
}

#[cfg(test)]
mod tests;
