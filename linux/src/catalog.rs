use std::sync::Arc;

use phonelink_engine::api::calling::{ActiveCallStatus, CallControlStatus, CallKind, CallsAccess};
use phonelink_engine::api::contacts::{ContactCardField, ContactCardNumber, ContactFieldLabel};
use phonelink_engine::api::engine::Engine;
use phonelink_engine::api::file_transfer::FileSendPhase;
use serde_json::{Value, json};

use crate::l10n::translate;
use crate::{images, mirror};

pub fn app_icon(engine: &Arc<Engine>, package: &str) -> Option<String> {
    images::cached("app-icons", package, || engine.clone().phone_apps().icon(package.to_owned()).ok().flatten())
}

fn contact_photo(engine: &Arc<Engine>, contact_id: i64) -> Option<String> {
    images::cached("contacts", &contact_id.to_string(), || {
        engine.clone().contacts().thumbnail(contact_id).ok().flatten()
    })
}

fn label(label: &ContactFieldLabel) -> String {
    let name = match label {
        ContactFieldLabel::Home => "Home",
        ContactFieldLabel::Mobile => "Mobile",
        ContactFieldLabel::Work => "Work",
        ContactFieldLabel::WorkMobile => "Work Mobile",
        ContactFieldLabel::Main => "Main",
        ContactFieldLabel::School => "School",
        ContactFieldLabel::Other => "Other",
        ContactFieldLabel::Birthday => "Birthday",
        ContactFieldLabel::Anniversary => "Anniversary",
        ContactFieldLabel::Custom { label } => return label.clone(),
        ContactFieldLabel::Generic => "",
    };
    translate(name).to_owned()
}

fn field(field: &ContactCardField) -> Value {
    json!({ "label": label(&field.label), "value": field.value })
}

fn number(number: &ContactCardNumber) -> Value {
    json!({ "label": label(&number.label), "value": number.shown, "number": number.number })
}

fn contacts(engine: &Arc<Engine>) -> Value {
    let list = engine.clone().contacts().list().unwrap_or_default();
    Value::Array(
        list.iter()
            .map(|c| {
                let photo = c.has_thumbnail.then(|| contact_photo(engine, c.contact_id)).flatten();
                json!({ "id": c.contact_id, "name": c.name, "photo": photo, "search": c.search_terms.join(" ") })
            })
            .collect(),
    )
}

fn contact(engine: &Arc<Engine>, arg: &str) -> Value {
    let Ok(id) = arg.parse::<i64>() else { return Value::Null };
    let Some(card) = engine.clone().contacts().card(id).ok().flatten() else { return Value::Null };
    let photo = card.has_thumbnail.then(|| contact_photo(engine, id)).flatten();
    json!({
        "id": card.contact_id,
        "name": card.name,
        "nickname": card.nickname,
        "company": card.company,
        "jobTitle": card.job_title,
        "notes": card.notes,
        "photo": photo,
        "phones": card.phone_numbers.iter().map(number).collect::<Vec<_>>(),
        "emails": card.email_addresses.iter().map(field).collect::<Vec<_>>(),
        "addresses": card.postal_addresses.iter().map(field).collect::<Vec<_>>(),
        "urls": card.urls.iter().map(field).collect::<Vec<_>>(),
        "dates": card.dates.iter().map(field).collect::<Vec<_>>(),
    })
}

fn photos(engine: &Arc<Engine>) -> Value {
    let api = engine.clone().photos();
    let gallery = api.gallery().unwrap_or_default();
    Value::Array(
        gallery
            .iter()
            .map(|p| {
                let thumbnail = p
                    .has_thumbnail
                    .then(|| {
                        images::cached("photo-thumbnails", &p.id.to_string(), || api.thumbnail(p.id).ok().flatten())
                    })
                    .flatten();
                json!({
                    "id": p.id,
                    "name": p.name,
                    "takenMs": p.taken_ms,
                    "width": p.width,
                    "height": p.height,
                    "thumbnail": thumbnail,
                    "hasFull": p.has_full,
                })
            })
            .collect(),
    )
}

fn photo(engine: &Arc<Engine>, arg: &str) -> Value {
    let Ok(id) = arg.parse::<i64>() else { return Value::Null };
    let api = engine.clone().photos();
    let _ = api.open_item(id);
    images::cached("photos", &id.to_string(), || api.full_image(id).ok().flatten()).map_or(Value::Null, Value::from)
}

fn call_kind(kind: CallKind) -> &'static str {
    match kind {
        CallKind::Incoming => "incoming",
        CallKind::Outgoing => "outgoing",
        CallKind::Missed => "missed",
        CallKind::Voicemail => "voicemail",
        CallKind::Rejected => "rejected",
        CallKind::Blocked => "blocked",
        CallKind::AnsweredExternally => "answeredExternally",
        CallKind::Unknown => "unknown",
    }
}

fn calls(engine: &Arc<Engine>) -> Value {
    let history = engine.clone().calling().history().unwrap_or_default();
    Value::Array(
        history
            .iter()
            .map(|c| {
                let photo = c.photo_contact_id.and_then(|id| contact_photo(engine, id));
                json!({
                    "id": c.call_id,
                    "number": c.number,
                    "name": c.name,
                    "contactId": c.contact_id,
                    "photo": photo,
                    "kind": call_kind(c.kind),
                    "startMs": c.start_ms,
                    "durationSecs": c.duration_secs,
                    "read": c.is_read,
                })
            })
            .collect(),
    )
}

fn apps(engine: &Arc<Engine>) -> Value {
    let list = engine.clone().phone_apps().apps().unwrap_or_default();
    Value::Array(
        list.iter()
            .map(|a| {
                let icon = a.has_icon.then(|| app_icon(engine, &a.package_name)).flatten();
                json!({ "package": a.package_name, "name": a.name, "icon": icon, "favorite": a.favorite })
            })
            .collect(),
    )
}

fn media(engine: &Arc<Engine>) -> Value {
    let Ok(controls) = engine.clone().audio().controls() else { return json!({ "shown": false }) };
    let session = controls.sessions.iter().find(|s| Some(s.id) == controls.selected_id).or(controls.sessions.first());
    let Some(session) = session.filter(|_| controls.shown) else { return json!({ "shown": false }) };
    let art = session.album_art.as_deref().and_then(|bytes| images::by_content("album-art", bytes));
    let icon = session.app_icon.as_deref().and_then(|bytes| images::by_content("media-app-icons", bytes));
    json!({
        "shown": true,
        "title": session.title,
        "artist": session.artist,
        "app": session.app_name,
        "art": art,
        "appIcon": icon,
        "playing": session.playing,
        "commanding": controls.commanding,
    })
}

fn transfers(engine: &Arc<Engine>) -> Value {
    let Ok(transfers) = engine.clone().file_transfer().transfers() else { return json!({}) };
    let send = transfers.send.map(|s| {
        let phase = match s.phase {
            FileSendPhase::Connecting => "connecting",
            FileSendPhase::Preparing => "preparing",
            FileSendPhase::Sending => "sending",
        };
        json!({ "phase": phase, "files": s.file_names, "done": s.bytes_done, "total": s.bytes_total })
    });
    let receive = transfers.receive.map(
        |r| json!({ "starting": r.starting, "files": r.file_names, "done": r.bytes_done, "total": r.bytes_total }),
    );
    json!({ "send": send, "receive": receive })
}

fn calls_state(engine: &Arc<Engine>) -> Value {
    let Ok(state) = engine.clone().calling().state() else { return json!({ "access": "unsupported" }) };
    let access = match state.access {
        CallsAccess::Unsupported => "unsupported",
        CallsAccess::PcConsentRequired => "consentRequired",
        CallsAccess::CallLogsUnsupported => "callLogsUnsupported",
        CallsAccess::PostNotificationRequired => "postNotificationRequired",
        CallsAccess::DevicePermissionRequired => "permissionRequired",
        CallsAccess::Granted => "granted",
    };
    json!({ "access": access, "refreshing": state.refreshing, "unseenMissed": state.unseen_missed })
}

fn call_control(engine: &Arc<Engine>) -> Value {
    let api = engine.clone().calling();
    let Ok(state) = api.control_state() else { return json!({ "offered": false }) };
    let status = match state.status {
        CallControlStatus::Off => "off",
        CallControlStatus::Starting => "starting",
        CallControlStatus::BluetoothOff => "bluetoothOff",
        CallControlStatus::NotPaired => "notPaired",
        CallControlStatus::Connecting => "connecting",
        CallControlStatus::ConnectionFailed => "connectionFailed",
        CallControlStatus::NotConnected => "notConnected",
        CallControlStatus::Ready => "ready",
    };
    let active: Vec<Value> = api
        .active_calls()
        .unwrap_or_default()
        .iter()
        .map(|call| {
            let call_status = match call.status {
                ActiveCallStatus::Incoming => "incoming",
                ActiveCallStatus::Dialing => "dialing",
                ActiveCallStatus::Talking => "talking",
                ActiveCallStatus::Held => "held",
            };
            json!({
                "id": call.id,
                "number": call.number,
                "name": call.name,
                "photo": call.photo_contact_id.and_then(|id| contact_photo(engine, id)),
                "status": call_status,
                "talkingSinceMs": call.talking_since_ms,
            })
        })
        .collect();
    json!({ "offered": state.offered, "enabled": state.enabled, "status": status, "active": active })
}

fn settings(engine: &Arc<Engine>) -> Value {
    let hotspot = engine.clone().hotspot().phone_state().ok().flatten();
    let calls = engine.clone().calling().control_state().ok();
    let clipboard = engine.clone().clipboard().phone_state().ok().flatten();
    let remote = engine.clone().remote_control().phone_state().ok().flatten();
    let files = engine.clone().file_transfer().settings().ok();
    json!({
        "clipboardApplies": clipboard.as_ref().is_some_and(|c| c.applicable),
        "clipboardSupported": clipboard.as_ref().is_some_and(|c| c.supported),
        "clipboard": clipboard.as_ref().is_some_and(|c| c.toggle && c.applicable),
        "remoteControlApplies": remote.as_ref().is_some_and(|r| r.applicable),
        "remoteControl": remote.as_ref().is_some_and(|r| r.toggle && r.applicable),
        "fileSharingShown": files.as_ref().is_some_and(|f| f.share_toggle_shown),
        "fileSharing": files.as_ref().is_some_and(|f| f.share_enabled),
        "canSendFiles": files.as_ref().is_some_and(|f| f.offered_as_send_target),
        "hotspotSupported": hotspot.as_ref().is_some_and(|h| h.supported),
        "hotspotBusy": hotspot.as_ref().is_some_and(|h| h.busy),
        "hotspotJoined": hotspot.as_ref().is_some_and(|h| h.joined),
        "callsOffered": calls.as_ref().is_some_and(|c| c.offered),
        "callsEnabled": calls.as_ref().is_some_and(|c| c.enabled),
    })
}

fn wallpaper(engine: &Arc<Engine>) -> Value {
    engine
        .clone()
        .wallpaper()
        .wallpaper()
        .ok()
        .flatten()
        .and_then(|w| images::by_content("wallpapers", &w.image))
        .map_or(Value::Null, Value::from)
}

fn phone_files(engine: &Arc<Engine>) -> Value {
    let Ok(state) = engine.clone().phone_files().state() else { return Value::Null };
    json!({
        "capable": state.capable,
        "enabled": state.enabled,
        "running": state.running,
        "permission": state.permission_granted,
        "ltwOff": state.ltw_off,
        "path": crate::phone_files::mount_point().to_string_lossy(),
    })
}

fn camera(engine: &Arc<Engine>) -> Value {
    let Ok(status) = engine.clone().connected_camera().status(String::from("en-US")) else { return Value::Null };
    json!({
        "available": status.mac_available,
        "supported": status.supported,
        "enabled": status.camera_enabled,
        "registered": status.registered,
        "registrationFailed": status.registration_failed,
        "permissionRequired": status.permission_required,
        "state": format!("{:?}", status.state),
        "error": format!("{:?}", status.error),
        "errorAction": format!("{:?}", status.error_action),
        "face": status.face.map(|face| format!("{face:?}")),
        "inUse": crate::camera::camera().in_use(),
        "controlWindow": status.control_window,
        "batteryLevel": status.battery_level,
        "charging": status.is_charging,
        "lowNetwork": status.low_network,
    })
}

pub fn query(engine: &Arc<Engine>, name: &str, arg: &str) -> Value {
    match name {
        "contacts" => contacts(engine),
        "contact" => contact(engine, arg),
        "photos" => photos(engine),
        "photo" => photo(engine, arg),
        "calls" => calls(engine),
        "callControl" => call_control(engine),
        "callsState" => calls_state(engine),
        "apps" => apps(engine),
        "appIcon" => app_icon(engine, arg).map_or(Value::Null, Value::from),
        "media" => media(engine),
        "transfers" => transfers(engine),
        "settings" => settings(engine),
        "wallpaper" => wallpaper(engine),
        "camera" => camera(engine),
        "phoneFiles" => phone_files(engine),
        "mirror.status" => mirror::views::status(engine),
        "mirror.sessions" => mirror::views::sessions(engine),
        _ => Value::Null,
    }
}
