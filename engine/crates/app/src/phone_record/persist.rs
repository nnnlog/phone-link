//! Storing the record and the identifiers it is kept under.

use super::{INSTALLATION_ID_KEY, PHONE_RECORD_KEY_PREFIX, PhoneRecord};
use crate::AppError;
use phonelink_core::edge::SecureStore;
use phonelink_core::types::DcgClient;

/// The store key of `phone`'s record. The id must be a DCG client id (letters, digits, `-`), so it
/// cannot reach outside the `app/phone/` namespace of a file-backed store.
pub fn record_key(phone: &DcgClient) -> Result<String, AppError> {
    let ok = !phone.id.is_empty() && phone.id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    if !ok {
        return Err(AppError::Programming("a phone id is not a DCG client id".to_owned()));
    }
    Ok(format!("{PHONE_RECORD_KEY_PREFIX}{}.{}", phone.id, phone.env))
}

pub fn load_record(store: &dyn SecureStore, phone: &DcgClient) -> Result<Option<PhoneRecord>, AppError> {
    let key = record_key(phone)?;
    let Some(bytes) = store.load(key.clone())? else {
        return Ok(None);
    };

    let record: PhoneRecord = serde_json::from_slice(&bytes)
        .map_err(|e| AppError::CorruptStore { key: key.clone(), reason: e.to_string() })?;

    if !record.phone_id.eq_ignore_ascii_case(&phone.id) || record.environment != phone.env {
        return Err(AppError::CorruptStore { key, reason: "record names another phone".into() });
    }
    Ok(Some(record))
}

pub fn save_record(store: &dyn SecureStore, record: &PhoneRecord) -> Result<(), AppError> {
    let key = record_key(&record.phone())?;
    let bytes = serde_json::to_vec(record)
        .map_err(|e| AppError::Programming(format!("phone record does not serialize: {e}")))?;
    store.store(key, bytes)?;
    Ok(())
}

/// `XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX`, upper-case hex, no braces.
pub fn is_installation_id(s: &str) -> bool {
    s.len() == 36
        && s.char_indices().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => c == '-',
            _ => c.is_ascii_digit() || ('A'..='F').contains(&c),
        })
}

/// The per-install `installationId`: generated on first use and persisted for the life of the
/// installation. A stored value of another shape is a corrupt store.
pub fn load_or_create_installation_id(store: &dyn SecureStore) -> Result<String, AppError> {
    match store.load(INSTALLATION_ID_KEY.into())? {
        Some(bytes) => {
            let id = String::from_utf8(bytes)
                .map_err(|e| AppError::CorruptStore { key: INSTALLATION_ID_KEY.into(), reason: e.to_string() })?;
            if !is_installation_id(&id) {
                return Err(AppError::CorruptStore {
                    key: INSTALLATION_ID_KEY.into(),
                    reason: "not an upper-case hyphenated GUID".into(),
                });
            }
            Ok(id)
        }
        None => {
            let id = uuid::Uuid::new_v4().hyphenated().to_string().to_uppercase();
            store.store(INSTALLATION_ID_KEY.into(), id.clone().into_bytes())?;
            Ok(id)
        }
    }
}

/// A fresh app session id: `{XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX}`, upper-case, with braces.
pub fn new_app_session_id() -> String {
    format!("{{{}}}", uuid::Uuid::new_v4().hyphenated().to_string().to_uppercase())
}

/// The app session id of this run (one per app run), minted on first use; every App layer of the
/// process — a rebuilt engine core's too — sends the same one.
pub fn process_app_session_id() -> &'static str {
    static APP_SESSION_ID: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    APP_SESSION_ID.get_or_init(new_app_session_id)
}
