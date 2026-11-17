use phonelink_engine::api::file_transfer::{
    FileReceiveProgress, FileSendFailure, FileSendPhase, FileSendProgress, FileSendResult, FileTransferSettings,
    FileTransfers,
};
use phonelink_engine::api::phone_files::PhoneFilesState;

use super::file_size::{SizeUnit, size_number};
use super::send_outcome::outcome_of;
use super::sending::file_paths;
use super::settings_view::{home_relative, recycle_bin_name, storage_status};
use super::transfer_view::{send_unavailable, transfer_view};
use crate::user_text;

fn send(phase: FileSendPhase, names: &[&str], done: u64, total: u64) -> FileSendProgress {
    FileSendProgress {
        phase,
        file_names: names.iter().map(|n| (*n).to_owned()).collect(),
        bytes_done: done,
        bytes_total: total,
    }
}

fn receive(starting: bool, names: &[&str], done: u64, total: u64) -> FileReceiveProgress {
    FileReceiveProgress {
        starting,
        file_names: names.iter().map(|n| (*n).to_owned()).collect(),
        bytes_done: done,
        bytes_total: total,
    }
}

#[test]
fn sizes_read_like_file_sizes() {
    assert_eq!(SizeUnit::of(0), SizeUnit::Bytes);
    assert_eq!(SizeUnit::of(999), SizeUnit::Bytes);
    assert_eq!(SizeUnit::of(1_000), SizeUnit::Kilo);
    assert_eq!(SizeUnit::of(999_499), SizeUnit::Kilo);
    // 999.5 KB rounds to 1000 KB, which reads as 1 MB.
    assert_eq!(SizeUnit::of(999_500), SizeUnit::Mega);
    assert_eq!(SizeUnit::of(3_400_000), SizeUnit::Mega);
    assert_eq!(SizeUnit::of(1_234_567_890), SizeUnit::Giga);
    assert_eq!(SizeUnit::of(5_000_000_000_000_000), SizeUnit::Tera);

    assert_eq!(size_number(999, SizeUnit::Bytes), "999");
    assert_eq!(size_number(1_500, SizeUnit::Kilo), "2");
    assert_eq!(size_number(3_400_000, SizeUnit::Mega), "3.4");
    assert_eq!(size_number(1_000_000, SizeUnit::Mega), "1");
    assert_eq!(size_number(1_234_567_890, SizeUnit::Giga), "1.23");
    assert_eq!(size_number(1_500_000_000, SizeUnit::Giga), "1.5");
    assert_eq!(size_number(0, SizeUnit::Mega), "0");
}

#[test]
fn a_send_reports_its_phase_then_its_bytes() {
    let transfers = |send| FileTransfers { send: Some(send), receive: None };
    let view = transfer_view(&transfers(send(FileSendPhase::Connecting, &["a.pdf"], 0, 0)), true);
    assert!(view.sending);
    assert_eq!(view.send_fraction, None);
    let card = view.card.expect("a card while sending");
    assert_eq!(card.name, "a.pdf");
    assert_eq!(card.amount, view.send_status);

    let view =
        transfer_view(&transfers(send(FileSendPhase::Sending, &["a.pdf", "b.jpg"], 3_400_000, 8_200_000)), false);
    assert_eq!(view.send_fraction, Some(3_400_000.0 / 8_200_000.0));
    let card = view.card.expect("a card while sending");
    assert_eq!(card.fraction, view.send_fraction);
    assert!(card.amount.is_some_and(|amount| amount.contains("3.4") && amount.contains("8.2")));
}

#[test]
fn a_window_send_shows_before_the_engine_reports_it() {
    let none = FileTransfers { send: None, receive: None };
    let view = transfer_view(&none, true);
    assert!(view.sending);
    assert!(view.send_status.is_some());
    assert!(view.card.is_some_and(|card| card.fraction.is_none()));

    let idle = transfer_view(&none, false);
    assert!(!idle.sending && idle.card.is_none() && idle.send_status.is_none() && idle.receive_status.is_none());
}

#[test]
fn a_receive_shows_its_files_and_progress() {
    let transfers = |receive| FileTransfers { send: None, receive: Some(receive) };
    let starting = transfer_view(&transfers(receive(true, &[], 0, 0)), false);
    let card = starting.card.expect("a card while receiving");
    assert_eq!((card.amount, card.fraction), (None, None));
    assert!(starting.receive_status.is_some());

    let running = transfer_view(&transfers(receive(false, &["x.jpg"], 500, 1_000)), false);
    let card = running.card.expect("a card while receiving");
    assert_eq!(card.name, "x.jpg");
    assert_eq!(card.fraction, Some(0.5));
    assert!(running.receive_status.is_some_and(|status| status.contains("x.jpg")));
}

#[test]
fn send_outcomes_say_why() {
    let sent = outcome_of(&FileSendResult::Sent { count: 2 });
    assert!(sent.succeeded && sent.message.contains('2'));

    let failed = |failure| outcome_of(&FileSendResult::Failed { failure, message: "the channel closed".to_owned() });
    assert!(failed(FileSendFailure::PermissionRequired).needs_permission);
    assert!(!failed(FileSendFailure::Metered).needs_permission);
    // The engine's own text is for the log.
    assert_eq!(failed(FileSendFailure::Failed).message, user_text::something_went_wrong());
    assert!(!failed(FileSendFailure::Cancelled).succeeded);
}

#[test]
fn sending_is_offered_only_to_a_capable_phone_that_allows_it() {
    let settings = |capable, offered| FileTransferSettings {
        share_capable: capable,
        share_toggle_shown: capable,
        share_enabled: offered,
        offered_as_send_target: offered,
        folder_setting_shown: false,
        received_files_folder: None,
    };
    assert!(send_unavailable(None).is_some());
    assert!(send_unavailable(Some(&settings(false, false))).is_some());
    assert!(send_unavailable(Some(&settings(true, false))).is_some());
    assert_eq!(send_unavailable(Some(&settings(true, true))), None);
}

#[test]
fn folders_in_the_home_folder_read_from_tilde() {
    let home = Some("/home/u");
    assert_eq!(home_relative("/home/u/Downloads/Mobile Devices", home), "~/Downloads/Mobile Devices");
    assert_eq!(home_relative("/home/u", home), "~");
    assert_eq!(home_relative("/home/user2/x", home), "/home/user2/x");
    assert_eq!(home_relative("/srv/x", None), "/srv/x");
}

#[test]
fn the_recycle_bin_is_named_by_its_last_folder() {
    assert_eq!(recycle_bin_name(Some("Internal storage\\Recycle Bin")), "Recycle Bin");
    assert_eq!(recycle_bin_name(Some("Trash")), "Trash");
}

#[test]
fn storage_reads_in_the_phones_gigabytes_while_the_files_run() {
    let state = |running, free, total| PhoneFilesState {
        domain: "phone".to_owned(),
        phone_name: "Galaxy".to_owned(),
        capable: true,
        enabled: true,
        running,
        permission_granted: true,
        available: true,
        sync: phonelink_engine::api::phone_files::PhoneFilesSync::Idle,
        initial_sync_done: true,
        storage_free: free,
        storage_total: total,
        gigabyte_bytes: 1_073_741_824,
        recycle_bin: None,
        ltw_off: false,
        recycle_notice_off: false,
    };
    let status = storage_status(&state(true, 26_843_545_600, 137_438_953_472)).expect("a storage line");
    assert!(status.contains("25.0") && status.contains("128.0"), "{status}");
    assert_eq!(storage_status(&state(false, 1, 2)), None);
    assert_eq!(storage_status(&state(true, 0, 0)), None);
}

#[test]
fn only_file_urls_are_sent() {
    assert_eq!(
        file_paths(r#"["file:///home/u/a%20b.pdf", "https://example.com/x"]"#),
        vec!["/home/u/a b.pdf".to_owned()]
    );
    assert!(file_paths("not json").is_empty());
}
