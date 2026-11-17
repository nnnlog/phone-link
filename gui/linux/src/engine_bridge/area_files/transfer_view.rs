//! The transfers as the sidebar's card and the Send Files window show them.
//!
//! The `transfers` query's JSON is built here.

use std::sync::Arc;

use phonelink_engine::api::engine::Engine;
use phonelink_engine::api::file_transfer::{
    FileReceiveProgress, FileSendPhase, FileSendProgress, FileTransferSettings, FileTransfers,
};
use serde_json::{Value, json};

use super::file_size::{amount, file_size};
use super::send_outcome::Outcome;
use super::state::State;
use crate::l10n::{fill, fill_plural, translate, translated};

/// The transfers as the sidebar's card and the Send Files window show them.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct TransferView {
    /// A send runs: the engine's, or one from the window that the engine has not reported yet.
    pub(super) sending: bool,
    /// What the running send is doing.
    pub(super) send_status: Option<String>,
    /// The running send's progress (0–1) while files are being sent.
    pub(super) send_fraction: Option<f64>,
    /// The running receive's status line.
    pub(super) receive_status: Option<String>,
    /// The card: shown while a send or a receive runs.
    pub(super) card: Option<Card>,
}

/// The sidebar's card of the running transfer.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Card {
    pub(super) name: String,
    pub(super) amount: Option<String>,
    pub(super) fraction: Option<f64>,
}

fn files_title(names: &[String]) -> Option<String> {
    match names {
        [] => None,
        [name] => Some(name.clone()),
        _ => Some(fill_plural("%lld files", names.len(), &[names.len().to_string()])),
    }
}

fn fraction(done: u64, total: u64) -> Option<f64> {
    (total > 0).then(|| done as f64 / total as f64)
}

pub(super) fn transfer_view(transfers: &FileTransfers, window_send: bool) -> TransferView {
    let send: Option<&FileSendProgress> = transfers.send.as_ref();
    let receive: Option<&FileReceiveProgress> = transfers.receive.as_ref();
    let sending = send.is_some() || window_send;

    let send_status = match send {
        None => window_send.then(|| translated("Connecting to your phone...")),
        Some(send) => Some(match send.phase {
            FileSendPhase::Connecting => translated("Connecting to your phone..."),
            FileSendPhase::Preparing => translated("Waiting for your phone..."),
            FileSendPhase::Sending => {
                fill(translate("Sending %@ of %@"), &[file_size(send.bytes_done), file_size(send.bytes_total)])
            }
        }),
    };
    let send_fraction = send
        .filter(|send| send.phase == FileSendPhase::Sending)
        .and_then(|send| fraction(send.bytes_done, send.bytes_total));
    let receive_status = receive.map(|receive| match receive.file_names.as_slice() {
        _ if receive.starting => translated("Your phone is sending files..."),
        [name] => fill(translate("Receiving %@"), std::slice::from_ref(name)),
        names => fill_plural("Receiving %lld files", names.len(), &[names.len().to_string()]),
    });

    let card = (sending || receive.is_some()).then(|| {
        let names = send.map(|s| &s.file_names).or(receive.map(|r| &r.file_names)).cloned().unwrap_or_default();
        let name = files_title(&names).unwrap_or_else(|| {
            if sending { translated("Sending to your phone") } else { translated("Receiving from your phone") }
        });
        let amount = match (send, receive) {
            (Some(send), _) if send.bytes_total > 0 => Some(amount(send.bytes_done, send.bytes_total)),
            (_, Some(receive)) if receive.bytes_total > 0 => Some(amount(receive.bytes_done, receive.bytes_total)),
            _ if sending => send_status.clone(),
            _ => None,
        };
        let fraction =
            if sending { send_fraction } else { receive.and_then(|r| fraction(r.bytes_done, r.bytes_total)) };
        Card { name, amount, fraction }
    });
    TransferView { sending, send_status, send_fraction, receive_status, card }
}

/// Why sending is not offered, for the Send Files window; `None` when it is.
pub(super) fn send_unavailable(settings: Option<&FileTransferSettings>) -> Option<String> {
    let Some(settings) = settings else { return Some(translated("Phone Link isn't connected to your phone yet.")) };
    if !settings.share_capable {
        return Some(translated(
            "Your phone can't receive files from this computer. Update Link to Windows on your phone.",
        ));
    }
    if !settings.offered_as_send_target {
        return Some(translated("Sending files to this phone is turned off in Settings › Files."));
    }
    None
}

/// The `transfers` query: the running transfers, and whether sending is offered.
///
/// The running send and receive come with how the card and the window show them.
pub(super) fn transfers(engine: &Arc<Engine>, state: &State) -> Value {
    let api = engine.clone().file_transfer();
    let settings = api.settings().ok();
    let transfers = api.transfers().unwrap_or(FileTransfers { send: None, receive: None });
    let view = transfer_view(&transfers, state.window_send);

    let send = transfers.send.as_ref().map(|s| {
        let phase = match s.phase {
            FileSendPhase::Connecting => "connecting",
            FileSendPhase::Preparing => "preparing",
            FileSendPhase::Sending => "sending",
        };
        json!({ "phase": phase, "files": s.file_names, "done": s.bytes_done, "total": s.bytes_total })
    });
    let receive = transfers.receive.as_ref().map(
        |r| json!({ "starting": r.starting, "files": r.file_names, "done": r.bytes_done, "total": r.bytes_total }),
    );
    let send_names = transfers.send.as_ref().map(|s| s.file_names.clone()).unwrap_or_default();
    json!({
        "send": send,
        "receive": receive,
        "sending": view.sending,
        "sendTitle": files_title(&send_names),
        "sendStatus": view.send_status,
        "sendFraction": view.send_fraction,
        "receiveStatus": view.receive_status,
        "card": view.card.map(|card| json!({ "name": card.name, "amount": card.amount, "fraction": card.fraction })),
        "canSend": settings.as_ref().is_some_and(|s| s.offered_as_send_target),
        "unavailable": send_unavailable(settings.as_ref()),
        "outcome": state.outcome.as_ref().map(Outcome::json),
    })
}
