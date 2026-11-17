//! The engine's secure store in the Secret Service (GNOME Keyring, `KWallet`, `KeePassXC`): one item
//! holding every key.

use std::collections::HashMap;
use std::sync::Mutex;

use base64::prelude::{BASE64_STANDARD, Engine as _};
use phonelink_core::sync::MutexExt;
use phonelink_engine::edge::{EdgeError, SecureStore};
use secret_service::EncryptionType;
use secret_service::blocking::{Collection, Item, SecretService};

#[cfg(test)]
mod tests;

const ITEM_LABEL: &str = "Phone Link engine store";
const ITEM_ATTRIBUTES: [(&str, &str); 2] = [("application", "phonelink"), ("item", "engine-store")];
const ITEM_CONTENT_TYPE: &str = "application/json";
/// The collection libsecret creates when no collection has the `default` alias, and the alias.
const DEFAULT_COLLECTION_LABEL: &str = "Default keyring";
const DEFAULT_ALIAS: &str = "default";

type KeyMap = HashMap<String, Vec<u8>>;

/// The engine's `SecureStore` in the Secret Service: one item holding every key as a JSON map of
/// base64 values. The map is read on first use and kept; every change writes the whole item through.
///
/// The item is looked for in every collection, and written back where it is; a first write puts it in
/// the default collection, which is created when there is none.
#[derive(Default)]
pub struct SecretServiceStore {
    entries: Mutex<Option<KeyMap>>,
}

impl SecretServiceStore {
    fn with_map<T>(&self, f: impl FnOnce(&mut KeyMap) -> Result<T, EdgeError>) -> Result<T, EdgeError> {
        let mut cached = self.entries.lock_or_recover();
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

/// The stored map; an empty one while no collection holds the item.
fn read_item() -> Result<KeyMap, EdgeError> {
    let service = SecretService::connect(EncryptionType::Dh).map_err(store_err)?;
    match stored_item(&service)? {
        Some(item) => decode_map(&item.get_secret().map_err(store_err)?),
        None => Ok(KeyMap::new()),
    }
}

/// Replaces the item's secret with `map`, in the collection that holds the item; without one, adds
/// the item to the default collection.
fn write_item(map: &KeyMap) -> Result<(), EdgeError> {
    let service = SecretService::connect(EncryptionType::Dh).map_err(store_err)?;
    let secret = encode_map(map);
    if let Some(item) = stored_item(&service)? {
        return item.set_secret(&secret, ITEM_CONTENT_TYPE).map_err(store_err);
    }
    let collection = default_collection(&service)?;
    if collection.is_locked().map_err(store_err)? {
        collection.unlock().map_err(store_err)?;
    }
    collection
        .create_item(ITEM_LABEL, HashMap::from(ITEM_ATTRIBUTES), &secret, true, ITEM_CONTENT_TYPE)
        .map_err(store_err)?;
    Ok(())
}

/// The store's item in whichever collection holds it, unlocked; `None` when no collection does.
/// Unlocking the item unlocks the collection that holds it, asking the user when the service needs to.
fn stored_item<'a>(service: &'a SecretService<'a>) -> Result<Option<Item<'a>>, EdgeError> {
    let found = service.search_items(HashMap::from(ITEM_ATTRIBUTES)).map_err(store_err)?;
    let Some(item) = only_item(found.unlocked.into_iter().chain(found.locked))? else { return Ok(None) };
    if item.is_locked().map_err(store_err)? {
        item.unlock().map_err(store_err)?;
    }
    Ok(Some(item))
}

/// The one item among `items`; `None` without any. Two would be two stores, and neither can be
/// chosen over the other.
fn only_item<T>(items: impl IntoIterator<Item = T>) -> Result<Option<T>, EdgeError> {
    let mut items = items.into_iter();
    let item = items.next();

    match items.count() {
        0 => Ok(item),
        more => Err(store_err(format!("{} items hold the engine store", more + 1))),
    }
}

/// The collection the `default` alias names; created as libsecret creates it when none has the alias.
fn default_collection<'a>(service: &'a SecretService<'a>) -> Result<Collection<'a>, EdgeError> {
    match service.get_default_collection() {
        Ok(collection) => Ok(collection),
        Err(secret_service::Error::NoResult) => {
            service.create_collection(DEFAULT_COLLECTION_LABEL, DEFAULT_ALIAS).map_err(store_err)
        }
        Err(e) => Err(store_err(e)),
    }
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
