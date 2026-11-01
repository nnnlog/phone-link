//! The metadata messages.

use super::{AnchorUpdate, MISSING, Parsed, acks, correlation_vector, opt, sequence_numbers};
use crate::caps::PhoneSettings;
use crate::consts::key;
use phonelink_app::Malformed;
use phonelink_core::codec::valueset::ValueSet;

/// One type's lists of a metadata message.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TypeLists {
    /// The incoming ids (`None`: the key is absent).
    pub ids: Option<Vec<i64>>,
    pub checksums: Option<Vec<i64>>,
    /// `messagingUnread<Type>Ids` (SMS and MMS only).
    pub unread: Option<Vec<i64>>,
}

impl TypeLists {
    /// The ids, absent ⇒ empty.
    pub fn ids(&self) -> &[i64] {
        self.ids.as_deref().unwrap_or(&[])
    }
}

/// What metadata parsing depends on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MetadataRules {
    /// RCS receive before the message's `settings` are applied: whether the RCS lists are read.
    pub rcs_receive_before: bool,
    /// RCS receive after the `settings`: whether the RCS types are synchronised and the RCS anchors read.
    pub rcs_receive_after: bool,
    /// The forced-resync condition (evaluated after the `settings`).
    pub forced_resync: bool,
}

/// A validated metadata message.
#[derive(Debug, Clone, PartialEq)]
pub struct Metadata {
    pub correlation_vector: String,
    pub settings: Option<PhoneSettings>,
    pub sms: TypeLists,
    pub mms: TypeLists,
    /// Read only when RCS receive held before the settings (else empty).
    pub rcs_chats: TypeLists,
    pub rcs_file_transfers: TypeLists,
    pub conversations: TypeLists,
    pub rcs_conversations: TypeLists,
    pub subscriptions: TypeLists,
    pub sequence_numbers: Option<AnchorUpdate>,
    pub acks: Option<Vec<i64>>,
}

pub(super) fn lists(payload: &ValueSet, ids: &'static str, checksums: &'static str) -> Parsed<TypeLists> {
    Ok(TypeLists {
        ids: opt::<&[i64]>(payload, ids)?.map(<[i64]>::to_vec),
        checksums: opt::<&[i64]>(payload, checksums)?.map(<[i64]>::to_vec),
        unread: None,
    })
}

/// Checksums the delta reads must be at least as long as the ids.
pub(super) fn checksums_cover(l: &TypeLists, k: &'static str) -> Parsed<()> {
    match &l.checksums {
        Some(c) if c.len() < l.ids().len() => {
            Err(Malformed::new("messaging checksum list shorter than its id list", k))
        }
        _ => Ok(()),
    }
}

/// Checksums required whenever the ids are non-empty, and at least as long.
pub(super) fn checksums_for_nonempty(l: &TypeLists, k: &'static str) -> Parsed<()> {
    if !l.ids().is_empty() && l.checksums.is_none() {
        return Err(Malformed::new("messaging checksums missing for non-empty ids", k));
    }
    checksums_cover(l, k)
}

/// Validates a metadata message under `rules`; `settings` is the message's
/// already-parsed `settings`.
pub fn metadata(payload: &ValueSet, settings: Option<PhoneSettings>, rules: MetadataRules) -> Parsed<Metadata> {
    let correlation_vector = correlation_vector(payload)?;
    let mut sms = lists(payload, key::SMS_IDS, key::SMS_CHECKSUMS)?;
    sms.unread = opt::<&[i64]>(payload, key::UNREAD_SMS_IDS)?.map(<[i64]>::to_vec);
    let mut mms = lists(payload, key::MMS_IDS, key::MMS_CHECKSUMS)?;
    mms.unread = opt::<&[i64]>(payload, key::UNREAD_MMS_IDS)?.map(<[i64]>::to_vec);

    let (rcs_chats, rcs_file_transfers) = if rules.rcs_receive_before {
        (
            lists(payload, key::RCS_CHAT_IDS, key::RCS_CHAT_CHECKSUMS)?,
            lists(payload, key::RCS_FILE_TRANSFER_IDS, key::RCS_FILE_TRANSFER_CHECKSUMS)?,
        )
    } else {
        (TypeLists::default(), TypeLists::default())
    };

    // Read and ignored.
    opt::<&[i64]>(payload, key::MMS_PART_IDS)?;

    let sms_or_mms = sms.ids.is_some() || mms.ids.is_some();
    let rcs_messages = rcs_chats.ids.is_some() || rcs_file_transfers.ids.is_some();
    let conversations = TypeLists {
        ids: match opt::<&[i64]>(payload, key::CONVERSATION_IDS)? {
            Some(v) => Some(v.to_vec()),
            None if sms_or_mms => return Err(Malformed::new(MISSING, key::CONVERSATION_IDS)),
            None => None,
        },
        checksums: opt::<&[i64]>(payload, key::CONVERSATION_CHECKSUMS)?.map(<[i64]>::to_vec),
        unread: None,
    };

    let rcs_conversations = if rules.rcs_receive_before {
        let read = |k: &'static str| -> Parsed<Option<Vec<i64>>> {
            match opt::<&[i64]>(payload, k)? {
                Some(v) => Ok(Some(v.to_vec())),
                None if rcs_messages => Err(Malformed::new(MISSING, k)),
                None => Ok(None),
            }
        };
        TypeLists {
            ids: read(key::RCS_CONVERSATION_IDS)?,
            checksums: read(key::RCS_CONVERSATION_CHECKSUMS)?,
            unread: None,
        }
    } else {
        TypeLists::default()
    };

    let subscriptions = TypeLists {
        ids: match opt::<&[i64]>(payload, key::SUBSCRIPTION_IDS)? {
            Some(v) => Some(v.to_vec()),
            None if sms_or_mms || rcs_messages => return Err(Malformed::new(MISSING, key::SUBSCRIPTION_IDS)),
            None => None,
        },
        checksums: opt::<&[i64]>(payload, key::SUBSCRIPTION_CHECKSUMS)?.map(<[i64]>::to_vec),
        unread: None,
    };

    // What the delta computation reads.
    if !rules.forced_resync {
        for (l, k) in [(&sms, key::SMS_CHECKSUMS), (&mms, key::MMS_CHECKSUMS)] {
            if l.checksums.is_none() {
                return Err(Malformed::new("messaging checksums missing outside a forced resync", k));
            }
            checksums_cover(l, k)?;
        }
        if rules.rcs_receive_after {
            checksums_for_nonempty(&rcs_chats, key::RCS_CHAT_CHECKSUMS)?;
            checksums_for_nonempty(&rcs_file_transfers, key::RCS_FILE_TRANSFER_CHECKSUMS)?;
        }
    }
    checksums_cover(&conversations, key::CONVERSATION_CHECKSUMS)?;
    if rules.rcs_receive_after {
        checksums_for_nonempty(&rcs_conversations, key::RCS_CONVERSATION_CHECKSUMS)?;
    }
    checksums_cover(&subscriptions, key::SUBSCRIPTION_CHECKSUMS)?;

    Ok(Metadata {
        correlation_vector,
        settings,
        sms,
        mms,
        rcs_chats,
        rcs_file_transfers,
        conversations,
        rcs_conversations,
        subscriptions,
        sequence_numbers: sequence_numbers(payload, rules.rcs_receive_after)?,
        acks: acks(payload)?,
    })
}
