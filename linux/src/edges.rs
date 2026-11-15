use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};

use base64::prelude::{BASE64_STANDARD, Engine as _};
use phonelink_engine::edge::{EdgeError, HostInfo, SecureStore};
use phonelink_engine::engine::{SystemNotification, SystemNotifier};
use secret_service::EncryptionType;
use secret_service::blocking::SecretService;

const ITEM_LABEL: &str = "Phone Link engine store";
const ITEM_ATTRIBUTES: [(&str, &str); 2] = [("application", "phonelink"), ("item", "engine-store")];

type KeyMap = HashMap<String, Vec<u8>>;

#[derive(Default)]
pub struct SecretServiceStore {
    entries: Mutex<Option<KeyMap>>,
}

impl SecretServiceStore {
    fn with_map<T>(&self, f: impl FnOnce(&mut KeyMap) -> Result<T, EdgeError>) -> Result<T, EdgeError> {
        let mut cached = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
        let map = match cached.as_mut() {
            Some(map) => map,
            None => cached.insert(read_item()?),
        };
        f(map)
    }
}

impl SecureStore for SecretServiceStore {
    fn load(&self, key: String) -> Result<Option<Vec<u8>>, EdgeError> {
        self.with_map(|map| Ok(map.get(&key).cloned()))
    }

    fn store(&self, key: String, value: Vec<u8>) -> Result<(), EdgeError> {
        self.with_map(|map| {
            let mut updated = map.clone();
            updated.insert(key, value);
            write_item(&updated)?;
            *map = updated;
            Ok(())
        })
    }

    fn delete(&self, key: String) -> Result<(), EdgeError> {
        self.with_map(|map| {
            if !map.contains_key(&key) {
                return Ok(());
            }
            let mut updated = map.clone();
            updated.remove(&key);
            write_item(&updated)?;
            *map = updated;
            Ok(())
        })
    }

    fn keys_with_prefix(&self, prefix: String) -> Result<Vec<String>, EdgeError> {
        self.with_map(|map| Ok(map.keys().filter(|key| key.starts_with(&prefix)).cloned().collect()))
    }
}

fn store_err(e: impl std::fmt::Display) -> EdgeError {
    EdgeError::Store { message: format!("Secret Service: {e}") }
}

fn read_item() -> Result<KeyMap, EdgeError> {
    let service = SecretService::connect(EncryptionType::Dh).map_err(store_err)?;
    let collection = service.get_default_collection().map_err(store_err)?;
    collection.ensure_unlocked().map_err(store_err)?;
    let items = collection.search_items(HashMap::from(ITEM_ATTRIBUTES)).map_err(store_err)?;
    let Some(item) = items.first() else {
        return Ok(KeyMap::new());
    };
    let secret = item.get_secret().map_err(store_err)?;
    decode_map(&secret)
}

fn write_item(map: &KeyMap) -> Result<(), EdgeError> {
    let service = SecretService::connect(EncryptionType::Dh).map_err(store_err)?;
    let collection = service.get_default_collection().map_err(store_err)?;
    collection.ensure_unlocked().map_err(store_err)?;
    collection
        .create_item(ITEM_LABEL, HashMap::from(ITEM_ATTRIBUTES), &encode_map(map), true, "application/json")
        .map_err(store_err)?;
    Ok(())
}

fn encode_map(map: &KeyMap) -> Vec<u8> {
    let encoded: HashMap<&str, String> =
        map.iter().map(|(key, value)| (key.as_str(), BASE64_STANDARD.encode(value))).collect();
    serde_json::to_vec(&encoded).expect("a map of strings serializes")
}

fn decode_map(bytes: &[u8]) -> Result<KeyMap, EdgeError> {
    let encoded: HashMap<String, String> =
        serde_json::from_slice(bytes).map_err(|e| store_err(format!("the item is not a key map: {e}")))?;
    encoded
        .into_iter()
        .map(|(key, value)| {
            let bytes = BASE64_STANDARD.decode(value).map_err(|e| store_err(format!("the value of {key}: {e}")))?;
            Ok((key, bytes))
        })
        .collect()
}

pub struct LinuxHost;

impl HostInfo for LinuxHost {
    fn host_name(&self) -> String {
        std::fs::read_to_string("/proc/sys/kernel/hostname").map(|name| name.trim().to_owned()).unwrap_or_default()
    }

    fn region_code(&self) -> Option<String> {
        ["LC_ALL", "LC_MESSAGES", "LANG"]
            .into_iter()
            .filter_map(|name| std::env::var(name).ok())
            .find(|value| !value.is_empty())
            .and_then(|locale| region_of_locale(&locale))
    }
}

fn region_of_locale(locale: &str) -> Option<String> {
    let region = locale.split(['.', '@']).next()?.split_once('_')?.1;
    (region.len() == 2 && region.chars().all(|c| c.is_ascii_alphabetic())).then(|| region.to_ascii_uppercase())
}

pub struct StderrNotifier;

impl SystemNotifier for StderrNotifier {
    fn post(&self, n: SystemNotification) -> Result<(), EdgeError> {
        eprintln!("[notify] {}: {} | {}", n.id, n.title, n.body);
        Ok(())
    }

    fn withdraw(&self, ids: Vec<String>) -> Result<(), EdgeError> {
        for id in ids {
            eprintln!("[notify withdraw] {id}");
        }
        Ok(())
    }

    fn is_authorized(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn region_comes_from_the_locale_territory() {
        assert_eq!(region_of_locale("ko_KR.UTF-8").as_deref(), Some("KR"));
        assert_eq!(region_of_locale("en_US@euro").as_deref(), Some("US"));
        assert_eq!(region_of_locale("C.UTF-8"), None);
    }

    #[test]
    fn key_map_round_trips() {
        let map = KeyMap::from([("auth/key".to_owned(), vec![0, 1, 255])]);
        assert_eq!(decode_map(&encode_map(&map)).unwrap(), map);
    }
}
