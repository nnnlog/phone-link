use phonelink_engine::api::device_status::{
    BatteryState, CellularState, DndDisplay, PhoneStatus, RingDisplay, RingerDisplay, WifiState,
};
use phonelink_engine::api::messaging::{Conversation, Message, MessageContact, MessageSendState};
use phonelink_engine::api::notifications::{NotificationLine, PhoneNotification};
use serde_json::{Value, json};

fn conversation_lines(lines: &[NotificationLine]) -> Vec<Value> {
    let mut result = Vec::new();
    let mut sender: Option<&str> = None;
    for line in lines {
        match line {
            NotificationLine::Sender { name } => {
                if let Some(previous) = sender.replace(name) {
                    result.push(json!({ "sender": previous, "text": "" }));
                }
            }
            NotificationLine::Message { text } => {
                result.push(json!({ "sender": sender.take().unwrap_or_default(), "text": text }));
            }
        }
    }
    if let Some(last) = sender {
        result.push(json!({ "sender": last, "text": "" }));
    }
    result
}

pub fn notifications_json(notifications: &[PhoneNotification]) -> String {
    let items: Vec<Value> = notifications
        .iter()
        .map(|n| {
            let conversation = conversation_lines(&n.conversation);
            let actions: Vec<Value> = n
                .actions
                .iter()
                .filter(|a| !a.inline_reply)
                .map(|a| json!({ "name": a.name, "index": a.index }))
                .collect();
            json!({
                "key": n.key,
                "app": n.app_name.clone().unwrap_or_else(|| n.package_name.clone()),
                "launchSupported": n.launch_supported,
                "package": n.package_name,
                "title": n.title.clone().unwrap_or_default(),
                "subtitle": n.subtitle.clone().unwrap_or_default(),
                "body": n.body,
                "conversation": conversation,
                "timeMs": n.time_ms,
                "pinned": n.pinned,
                "replyIndex": n.reply.as_ref().map(|r| r.index),
                "replyName": n.reply.as_ref().map(|r| r.name.clone()),
                "actions": actions,
            })
        })
        .collect();
    Value::Array(items).to_string()
}

pub fn status_json(status: &PhoneStatus) -> String {
    let (battery, charging) = match status.battery {
        BatteryState::Level { percent, charging, .. } => (Some(percent), charging),
        BatteryState::Unknown => (None, false),
    };
    let wifi_bars = match status.wifi {
        WifiState::Bars { level } => Some(level),
        WifiState::NotConnected | WifiState::Unknown => None,
    };
    let (cell_bars, cell_max) = match &status.cellular {
        CellularState::Bars { level, max, .. } => (Some(*level), Some(*max)),
        _ => (None, None),
    };
    let dnd = status.do_not_disturb.map(|control| {
        let shown = match control.shown {
            DndDisplay::Status { on: true, .. } => "on",
            DndDisplay::Status { on: false, .. } => "off",
            DndDisplay::TurningOn => "turningOn",
            DndDisplay::TurningOff => "turningOff",
            DndDisplay::Unknown => "unknown",
        };
        json!({ "shown": shown, "enabled": control.enabled })
    });
    let ringer = status.ringer.map(|control| {
        let shown = match control.shown {
            RingerDisplay::Mode { mode: 0 } => "silent",
            RingerDisplay::Mode { mode: 1 } => "vibrate",
            RingerDisplay::Mode { .. } => "sound",
            RingerDisplay::TurningOn | RingerDisplay::TurningOff | RingerDisplay::TurningVibrate => "changing",
            RingerDisplay::Unknown => "unknown",
        };
        json!({ "shown": shown, "enabled": control.enabled })
    });
    let ring = status.ring_my_phone.map(|control| {
        let ringing = matches!(control.shown, RingDisplay::Ringing { .. });
        json!({ "ringing": ringing, "enabled": control.enabled, "canStop": control.can_stop })
    });
    json!({
        "live": status.live,
        "battery": battery,
        "charging": charging,
        "wifiBars": wifi_bars,
        "cellBars": cell_bars,
        "cellMax": cell_max,
        "dnd": dnd,
        "ringer": ringer,
        "ring": ring,
    })
    .to_string()
}

fn contact_name(contact: &MessageContact) -> String {
    contact.name.clone().filter(|name| !name.trim().is_empty()).unwrap_or_else(|| contact.address.clone())
}

pub fn conversations_json(conversations: &[Conversation]) -> String {
    let items: Vec<Value> = conversations
        .iter()
        .filter(|c| !c.hidden)
        .map(|c| {
            let names: Vec<String> = c.recipients.iter().map(contact_name).collect();
            let preview = c
                .preview
                .as_ref()
                .map(|p| if p.body.is_empty() && p.attachment { "Attachment".to_owned() } else { p.body.clone() });
            json!({
                "threadId": c.thread_id,
                "name": names.join(", "),
                "address": if c.is_group { String::new() } else { c.recipients.first().map(|r| r.address.clone()).unwrap_or_default() },
                "isGroup": c.is_group,
                "preview": preview.unwrap_or_default(),
                "latestMs": c.latest_ms,
                "unread": c.unread,
                "canReply": c.can_reply,
            })
        })
        .collect();
    Value::Array(items).to_string()
}

pub fn messages_json(messages: &[Message]) -> String {
    let items: Vec<Value> = messages
        .iter()
        .map(|m| {
            json!({
                "id": m.id,
                "incoming": m.incoming,
                "sender": contact_name(&m.contact),
                "body": m.body,
                "timestampMs": m.timestamp_ms,
                "attachments": m.parts.len(),
                "sendState": m.send_state.map(|state| match state {
                    MessageSendState::Sending => "sending",
                    MessageSendState::Sent => "sent",
                    MessageSendState::Failed { .. } => "failed",
                }),
            })
        })
        .collect();
    Value::Array(items).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contact(address: &str, name: Option<&str>) -> MessageContact {
        MessageContact {
            address: address.to_owned(),
            name: name.map(str::to_owned),
            contact_id: None,
            photo_contact_id: None,
        }
    }

    #[test]
    fn conversation_pairs_each_message_with_its_sender() {
        let lines = [
            NotificationLine::Sender { name: "Mina".to_owned() },
            NotificationLine::Message { text: "hi".to_owned() },
            NotificationLine::Message { text: "again".to_owned() },
        ];
        assert_eq!(
            conversation_lines(&lines),
            [json!({ "sender": "Mina", "text": "hi" }), json!({ "sender": "", "text": "again" })]
        );
    }

    #[test]
    fn contact_name_falls_back_to_the_address() {
        assert_eq!(contact_name(&contact("+8210", Some("Kim"))), "Kim");
        assert_eq!(contact_name(&contact("+8210", Some(" "))), "+8210");
        assert_eq!(contact_name(&contact("+8210", None)), "+8210");
    }
}
