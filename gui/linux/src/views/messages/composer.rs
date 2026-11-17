//! What the Messages section's detail column shows and where its composer sends: a conversation, the
//! new message and its recipients, or what was sent to new recipients; and the composer's length limit.

/// A recipient of a new message: the address as the user entered or picked it (the engine sends it
/// as it is), and the contact's name when it is known.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recipient {
    pub address: String,
    pub name: Option<String>,
}

impl Recipient {
    pub fn title(&self) -> &str {
        self.name.as_deref().filter(|name| !name.trim().is_empty()).unwrap_or(&self.address)
    }
}

/// What the detail column shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Detail {
    /// A conversation of the phone.
    Conversation(i64),
    /// The new message being written: its recipients can still change.
    NewMessage,
    /// What was sent to these recipients before the phone created their conversation: the sends with
    /// their state, and a composer that sends to the same recipients.
    Unsent(Vec<Recipient>),
}

/// Where a composer sends, which keys its draft.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ComposerTarget {
    Conversation(i64),
    NewMessage,
    Recipients(Vec<String>),
}

impl ComposerTarget {
    /// A key the window tells drafts apart by.
    pub fn key(&self) -> String {
        match self {
            Self::Conversation(thread_id) => format!("conversation:{thread_id}"),
            Self::NewMessage => "new".to_owned(),
            Self::Recipients(addresses) => format!("recipients:{}", addresses.join("\n")),
        }
    }
}

/// `text` cut to at most `max` UTF-16 code units — the engine's count — at a character boundary.
pub fn clamp_utf16(text: &str, max: Option<usize>) -> String {
    let Some(max) = max else { return text.to_owned() };
    let mut count = 0;
    let mut kept = String::new();
    for c in text.chars() {
        count += c.len_utf16();
        if count > max {
            break;
        }
        kept.push(c);
    }
    kept
}
