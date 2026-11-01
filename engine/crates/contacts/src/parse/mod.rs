//! Validation of inbound `contactsV2` messages into typed values, before anything is applied.
//!
//! It covers the metadata and the items: the batch rules of all six batch types, the `contacts`
//! batch and the sub-record batches.
//!
//! Every message that breaks a rule is refused whole with a fixed English reason (Phone Link on
//! Windows does not answer it). A `contactSettings`, `sequenceNumbers` or batch key that is not an
//! `Object`, and keys not read, are ignored.

use phonelink_app::Malformed;
use phonelink_core::codec::valueset::{Value, ValueSet};

use super::wire::{RecordType, contact_key, date_key, email_key, key, phone_key, postal_key, sync_action, url_key};

/// Unix milliseconds accepted for `lastUpdatedTimes` (0001-01-01 … 9999-12-31T23:59:59.999Z).
pub const MIN_UNIX_MS: i64 = -62_135_596_800_000;
pub const MAX_UNIX_MS: i64 = 253_402_300_799_999;

/// A key of another type than the contract's.
fn mistyped(key: &'static str) -> Malformed {
    Malformed::new("contactsV2 key of another type", key)
}

/// The six sequence numbers of one `sequenceNumbers` object, by [`RecordType`]; `-1` = none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SequenceNumbers(pub [i64; 6]);

impl SequenceNumbers {
    /// Every type `-1` (what a session holds before it sees a `sequenceNumbers` object).
    pub const NONE: SequenceNumbers = SequenceNumbers([-1; 6]);

    pub fn get(&self, t: RecordType) -> i64 {
        self.0[t.index()]
    }
}

/// A `sequenceNumbers` `Object`: each of the six keys `Int64` when present, `-1` when missing.
/// Absent, or not an `Object`: `None` (ignored).
pub fn parse_sequence_numbers(payload: &ValueSet) -> Result<Option<SequenceNumbers>, Malformed> {
    let Some(Value::Object(object)) = payload.get(key::SEQUENCE_NUMBERS) else {
        return Ok(None);
    };

    let mut values = [-1i64; 6];
    for t in RecordType::ALL {
        values[t.index()] = match object.get(t.sequence_key()) {
            None => -1,
            Some(Value::Int64(v)) => *v,
            Some(_) => return Err(mistyped(t.sequence_key())),
        };
    }
    Ok(Some(SequenceNumbers(values)))
}

fn optional_i64_array<'a>(vs: &'a ValueSet, k: &'static str) -> Result<Option<&'a [i64]>, Malformed> {
    match vs.get(k) {
        None => Ok(None),
        Some(Value::Int64Array(v)) => Ok(Some(v)),
        Some(_) => Err(mistyped(k)),
    }
}

// ------------------------------------------------------------------------------------------------
// Metadata message
// ------------------------------------------------------------------------------------------------

/// A validated metadata message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetadataMessage {
    /// The incoming ids `I` of each type (empty when the key is absent).
    pub ids: [Vec<i64>; 6],
    /// `C[i]` for every `i` of `ids` (all `0` when the checksum key is absent).
    pub checksums: [Vec<i64>; 6],
    /// `contactSettings` was an `Object`: its `contactCapabilities`, `0` (`None`) when missing.
    pub contact_capabilities: Option<i32>,
    pub sequence_numbers: Option<SequenceNumbers>,
}

/// Validates a metadata message (the payload has a `metadata` key).
pub fn parse_metadata(payload: &ValueSet) -> Result<MetadataMessage, Malformed> {
    let mut ids: [Vec<i64>; 6] = Default::default();
    let mut checksums: [Vec<i64>; 6] = Default::default();
    for t in RecordType::ALL {
        let incoming = optional_i64_array(payload, t.ids_key())?.unwrap_or_default();
        let sums = match optional_i64_array(payload, t.checksums_key())? {
            None => vec![0; incoming.len()],
            Some(c) if c.len() < incoming.len() => {
                return Err(Malformed::new("contactsV2 checksums shorter than ids", t.checksums_key()));
            }
            Some(c) => c[..incoming.len()].to_vec(),
        };
        ids[t.index()] = incoming.to_vec();
        checksums[t.index()] = sums;
    }

    let contact_capabilities = match payload.get(key::CONTACT_SETTINGS) {
        Some(Value::Object(settings)) => match settings.get(key::CONTACT_CAPABILITIES) {
            None => Some(super::wire::contacts_capabilities::NONE),
            Some(Value::Int32(v)) => Some(*v),
            Some(_) => return Err(mistyped(key::CONTACT_CAPABILITIES)),
        },
        _ => None,
    };
    let sequence_numbers = parse_sequence_numbers(payload)?;
    Ok(MetadataMessage { ids, checksums, contact_capabilities, sequence_numbers })
}

// ------------------------------------------------------------------------------------------------
// Item message
// ------------------------------------------------------------------------------------------------

/// A contact as an item message carries it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContactRecord {
    pub id: i64,
    pub checksum: i64,
    /// Unix epoch milliseconds.
    pub last_updated_ms: i64,
    pub display_name: String,
    pub nickname: String,
    pub company: String,
    pub job_title: String,
    pub notes: String,
    pub name_prefix: String,
    pub name_suffix: String,
    pub given_name: String,
    pub middle_name: String,
    pub family_name: String,
    /// The entry's slice of `thumbnails`; `None` keeps a stored thumbnail.
    pub thumbnail: Option<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhoneNumberRecord {
    pub id: i64,
    pub contact_id: i64,
    pub checksum: i64,
    pub number: String,
    /// Empty = absent.
    pub display_number: String,
    pub kind: i32,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmailAddressRecord {
    pub id: i64,
    pub contact_id: i64,
    pub checksum: i64,
    pub address: String,
    pub kind: i32,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PostalAddressRecord {
    pub id: i64,
    pub contact_id: i64,
    pub checksum: i64,
    pub kind: i32,
    pub label: String,
    pub display_address: String,
    pub street: String,
    pub city: String,
    pub region: String,
    pub postal_code: String,
    pub country_code: String,
    pub sub_administrative_area: String,
    pub sub_locality: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContactDateRecord {
    pub id: i64,
    pub contact_id: i64,
    pub checksum: i64,
    pub kind: i32,
    pub label: String,
    pub display_date: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContactUrlRecord {
    pub id: i64,
    pub contact_id: i64,
    pub checksum: i64,
    pub kind: i32,
    pub label: String,
    pub address: String,
}

/// One processed batch: its deletes (applied first), its upserts in entry order, and every processed
/// id (deleted or not), which leaves its type's pending set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Batch<R> {
    pub deletes: Vec<i64>,
    pub upserts: Vec<R>,
    pub processed: Vec<i64>,
}

impl<R> Default for Batch<R> {
    fn default() -> Self {
        Self { deletes: Vec::new(), upserts: Vec::new(), processed: Vec::new() }
    }
}

impl<R> Batch<R> {
    /// Entry `i` with id `id`: a delete for `SyncAction` `2`, else the upsert `record()` builds.
    fn entry(&mut self, id: i64, action: i32, record: impl FnOnce() -> Result<R, Malformed>) -> Result<(), Malformed> {
        self.processed.push(id);
        if action == sync_action::DELETE {
            self.deletes.push(id);
        } else {
            self.upserts.push(record()?);
        }
        Ok(())
    }
}

/// A validated item message. A batch is `None` when absent, not an `Object`, or `itemCount` `0`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ItemMessage {
    pub contacts: Option<Batch<ContactRecord>>,
    pub phone_numbers: Option<Batch<PhoneNumberRecord>>,
    pub email_addresses: Option<Batch<EmailAddressRecord>>,
    pub postal_addresses: Option<Batch<PostalAddressRecord>>,
    pub contact_dates: Option<Batch<ContactDateRecord>>,
    pub contact_urls: Option<Batch<ContactUrlRecord>>,
    pub sequence_numbers: Option<SequenceNumbers>,
}

impl ItemMessage {
    /// The ids of type `t` this message processed.
    pub fn processed(&self, t: RecordType) -> &[i64] {
        fn ids<R>(b: &Option<Batch<R>>) -> &[i64] {
            b.as_ref().map_or(&[], |b| b.processed.as_slice())
        }
        match t {
            RecordType::Contact => ids(&self.contacts),
            RecordType::PhoneNumber => ids(&self.phone_numbers),
            RecordType::EmailAddress => ids(&self.email_addresses),
            RecordType::PostalAddress => ids(&self.postal_addresses),
            RecordType::ContactDate => ids(&self.contact_dates),
            RecordType::ContactUrl => ids(&self.contact_urls),
        }
    }

    /// Whether any batch has an entry (so the store changes).
    pub fn has_entries(&self) -> bool {
        RecordType::ALL.iter().any(|t| !self.processed(*t).is_empty())
    }
}

/// Validates an item message (no `metadata` key).
pub fn parse_item(payload: &ValueSet) -> Result<ItemMessage, Malformed> {
    Ok(ItemMessage {
        contacts: parse_contacts(payload)?,
        phone_numbers: parse_phone_numbers(payload)?,
        email_addresses: parse_email_addresses(payload)?,
        postal_addresses: parse_postal_addresses(payload)?,
        contact_dates: parse_contact_dates(payload)?,
        contact_urls: parse_contact_urls(payload)?,
        sequence_numbers: parse_sequence_numbers(payload)?,
    })
}

/// Reads one batch's per-entry arrays and computes `n` = the minimum of every present array's
/// length and of `itemCount` for every absent optional array.
struct BatchReader<'a> {
    batch: &'a ValueSet,
    item_count: i32,
    n: i64,
}

impl<'a> BatchReader<'a> {
    /// The batch under `batch_key`: `None` when absent or not an `Object` (ignored) and when
    /// `itemCount` is `0` (skipped); `itemCount` must be an `Int32` otherwise.
    fn open(item: &'a ValueSet, batch_key: &'static str) -> Result<Option<Self>, Malformed> {
        let Some(Value::Object(batch)) = item.get(batch_key) else {
            return Ok(None);
        };

        let item_count = match batch.get(key::ITEM_COUNT) {
            None => return Err(Malformed::new("contactsV2 batch without itemCount", batch_key)),
            Some(Value::Int32(n)) => *n,
            Some(_) => return Err(mistyped(key::ITEM_COUNT)),
        };

        if item_count == 0 {
            return Ok(None);
        }
        Ok(Some(Self { batch, item_count, n: i64::MAX }))
    }

    fn present(&mut self, len: usize) {
        self.n = self.n.min(len as i64);
    }

    fn absent(&mut self) {
        self.n = self.n.min(i64::from(self.item_count));
    }

    fn required_i64(&mut self, k: &'static str) -> Result<&'a [i64], Malformed> {
        match self.batch.get(k) {
            None => Err(Malformed::new("contactsV2 batch without a required array", k)),
            Some(Value::Int64Array(v)) => {
                self.present(v.len());
                Ok(v)
            }
            Some(_) => Err(mistyped(k)),
        }
    }

    fn optional_i64(&mut self, k: &'static str) -> Result<Option<&'a [i64]>, Malformed> {
        match self.batch.get(k) {
            None => {
                self.absent();
                Ok(None)
            }
            Some(Value::Int64Array(v)) => {
                self.present(v.len());
                Ok(Some(v))
            }
            Some(_) => Err(mistyped(k)),
        }
    }

    fn optional_i32(&mut self, k: &'static str) -> Result<Option<&'a [i32]>, Malformed> {
        match self.batch.get(k) {
            None => {
                self.absent();
                Ok(None)
            }
            Some(Value::Int32Array(v)) => {
                self.present(v.len());
                Ok(Some(v))
            }
            Some(_) => Err(mistyped(k)),
        }
    }

    fn optional_strings(&mut self, k: &'static str) -> Result<Option<&'a [String]>, Malformed> {
        match self.batch.get(k) {
            None => {
                self.absent();
                Ok(None)
            }
            Some(Value::StringArray(v)) => {
                self.present(v.len());
                Ok(Some(v))
            }
            Some(_) => Err(mistyped(k)),
        }
    }

    /// The number of entries to process; fewer than `itemCount` refuses the message.
    fn entries(&self, batch_key: &'static str) -> Result<usize, Malformed> {
        if self.n < i64::from(self.item_count) {
            return Err(Malformed::new("contactsV2 batch with fewer entries than itemCount", batch_key));
        }
        // A negative `itemCount` with an absent optional array processes nothing.
        Ok(usize::try_from(self.n.max(0)).unwrap_or(usize::MAX))
    }
}

fn text(a: Option<&[String]>, i: usize) -> String {
    a.map(|a| a[i].clone()).unwrap_or_default()
}

fn int(a: Option<&[i32]>, i: usize) -> i32 {
    a.map_or(0, |a| a[i])
}

fn long(a: Option<&[i64]>, i: usize) -> i64 {
    a.map_or(0, |a| a[i])
}

fn action(a: Option<&[i32]>, i: usize) -> i32 {
    a.map_or(sync_action::SYNC, |a| a[i])
}

fn parse_contacts(item: &ValueSet) -> Result<Option<Batch<ContactRecord>>, Malformed> {
    use contact_key as k;
    let batch_key = RecordType::Contact.batch_key();
    let Some(mut r) = BatchReader::open(item, batch_key)? else {
        return Ok(None);
    };

    let ids = r.required_i64(k::CONTACT_IDS)?;
    let times = r.required_i64(k::LAST_UPDATED_TIMES)?;
    let checksums = r.optional_i64(key::CHECKSUMS)?;
    let display_names = r.optional_strings(k::DISPLAY_NAMES)?;
    let nicknames = r.optional_strings(k::NICKNAMES)?;
    let companies = r.optional_strings(k::COMPANIES)?;
    let job_titles = r.optional_strings(k::JOB_TITLES)?;
    let notes = r.optional_strings(k::NOTES)?;
    let name_prefixes = r.optional_strings(k::NAME_PREFIXES)?;
    let name_suffixes = r.optional_strings(k::NAME_SUFFIXES)?;
    let given_names = r.optional_strings(k::GIVEN_NAMES)?;
    let middle_names = r.optional_strings(k::MIDDLE_NAMES)?;
    let family_names = r.optional_strings(k::FAMILY_NAMES)?;
    let sizes = r.optional_i64(k::THUMBNAIL_SIZES)?;
    let actions = r.optional_i32(key::ACTIONS)?;
    // Not per-entry: one blob consumed by a running offset over the non-deleted entries.
    let blob: &[u8] = match r.batch.get(k::THUMBNAILS) {
        None => &[],
        Some(Value::UInt8Array(b)) => b,
        Some(_) => return Err(mistyped(k::THUMBNAILS)),
    };
    let n = r.entries(batch_key)?;

    let mut batch = Batch::default();
    let mut offset = 0usize;
    for i in 0..n {
        batch.entry(ids[i], action(actions, i), || {
            let last_updated_ms = times[i];
            if !(MIN_UNIX_MS..=MAX_UNIX_MS).contains(&last_updated_ms) {
                return Err(Malformed::new("contactsV2 lastUpdatedTimes out of range", k::LAST_UPDATED_TIMES));
            }

            let size = long(sizes, i);
            let thumbnail = if size > 0 {
                let size = i32::try_from(size)
                    .map_err(|_| Malformed::new("contactsV2 thumbnail size out of range", k::THUMBNAIL_SIZES))?
                    as usize;
                let end = offset
                    .checked_add(size)
                    .filter(|end| *end <= blob.len())
                    .ok_or(Malformed::new("contactsV2 thumbnails shorter than thumbnailSizes", k::THUMBNAILS))?;
                let bytes = blob[offset..end].to_vec();
                offset = end;
                Some(bytes)
            } else {
                None
            };

            Ok(ContactRecord {
                id: ids[i],
                checksum: long(checksums, i),
                last_updated_ms,
                display_name: text(display_names, i),
                nickname: text(nicknames, i),
                company: text(companies, i),
                job_title: text(job_titles, i),
                notes: text(notes, i),
                name_prefix: text(name_prefixes, i),
                name_suffix: text(name_suffixes, i),
                given_name: text(given_names, i),
                middle_name: text(middle_names, i),
                family_name: text(family_names, i),
                thumbnail,
            })
        })?;
    }
    Ok(Some(batch))
}

fn parse_phone_numbers(item: &ValueSet) -> Result<Option<Batch<PhoneNumberRecord>>, Malformed> {
    use phone_key as k;
    let batch_key = RecordType::PhoneNumber.batch_key();
    let Some(mut r) = BatchReader::open(item, batch_key)? else {
        return Ok(None);
    };

    let ids = r.required_i64(k::PHONE_NUMBER_IDS)?;
    let contact_ids = r.required_i64(key::CONTACT_IDS)?;
    let checksums = r.optional_i64(key::CHECKSUMS)?;
    let numbers = r.optional_strings(k::PHONE_NUMBERS)?;
    let display_numbers = r.optional_strings(k::DISPLAY_NUMBERS)?;
    let types = r.optional_i32(k::TYPES)?;
    let labels = r.optional_strings(k::LABELS)?;
    let actions = r.optional_i32(key::ACTIONS)?;
    let n = r.entries(batch_key)?;

    let mut batch = Batch::default();
    for i in 0..n {
        batch.entry(ids[i], action(actions, i), || {
            Ok(PhoneNumberRecord {
                id: ids[i],
                contact_id: contact_ids[i],
                checksum: long(checksums, i),
                number: text(numbers, i),
                display_number: text(display_numbers, i),
                kind: int(types, i),
                label: text(labels, i),
            })
        })?;
    }
    Ok(Some(batch))
}

fn parse_email_addresses(item: &ValueSet) -> Result<Option<Batch<EmailAddressRecord>>, Malformed> {
    use email_key as k;
    let batch_key = RecordType::EmailAddress.batch_key();
    let Some(mut r) = BatchReader::open(item, batch_key)? else {
        return Ok(None);
    };

    let ids = r.required_i64(k::IDS)?;
    let contact_ids = r.required_i64(key::CONTACT_IDS)?;
    let checksums = r.optional_i64(key::CHECKSUMS)?;
    let addresses = r.optional_strings(k::ADDRESSES)?;
    let types = r.optional_i32(k::TYPE)?;
    let labels = r.optional_strings(k::LABEL)?;
    let actions = r.optional_i32(key::ACTIONS)?;
    let n = r.entries(batch_key)?;

    let mut batch = Batch::default();
    for i in 0..n {
        batch.entry(ids[i], action(actions, i), || {
            Ok(EmailAddressRecord {
                id: ids[i],
                contact_id: contact_ids[i],
                checksum: long(checksums, i),
                address: text(addresses, i),
                kind: int(types, i),
                label: text(labels, i),
            })
        })?;
    }
    Ok(Some(batch))
}

fn parse_postal_addresses(item: &ValueSet) -> Result<Option<Batch<PostalAddressRecord>>, Malformed> {
    use postal_key as k;
    let batch_key = RecordType::PostalAddress.batch_key();
    let Some(mut r) = BatchReader::open(item, batch_key)? else {
        return Ok(None);
    };

    let ids = r.required_i64(k::IDS)?;
    let contact_ids = r.required_i64(key::CONTACT_IDS)?;
    let checksums = r.optional_i64(key::CHECKSUMS)?;
    let types = r.optional_i32(k::TYPE)?;
    let labels = r.optional_strings(k::LABEL)?;
    let display_addresses = r.optional_strings(k::DISPLAY_ADDRESS)?;
    let streets = r.optional_strings(k::STREET)?;
    let cities = r.optional_strings(k::CITY)?;
    let regions = r.optional_strings(k::REGION)?;
    let postal_codes = r.optional_strings(k::POSTAL_CODE)?;
    let country_codes = r.optional_strings(k::COUNTRY_CODE)?;
    let sub_administrative_areas = r.optional_strings(k::SUB_ADMINISTRATIVE_AREA)?;
    let sub_localities = r.optional_strings(k::SUB_LOCALITY)?;
    let actions = r.optional_i32(key::ACTIONS)?;
    let n = r.entries(batch_key)?;

    let mut batch = Batch::default();
    for i in 0..n {
        batch.entry(ids[i], action(actions, i), || {
            Ok(PostalAddressRecord {
                id: ids[i],
                contact_id: contact_ids[i],
                checksum: long(checksums, i),
                kind: int(types, i),
                label: text(labels, i),
                display_address: text(display_addresses, i),
                street: text(streets, i),
                city: text(cities, i),
                region: text(regions, i),
                postal_code: text(postal_codes, i),
                country_code: text(country_codes, i),
                sub_administrative_area: text(sub_administrative_areas, i),
                sub_locality: text(sub_localities, i),
            })
        })?;
    }
    Ok(Some(batch))
}

fn parse_contact_dates(item: &ValueSet) -> Result<Option<Batch<ContactDateRecord>>, Malformed> {
    use date_key as k;
    let batch_key = RecordType::ContactDate.batch_key();
    let Some(mut r) = BatchReader::open(item, batch_key)? else {
        return Ok(None);
    };

    let ids = r.required_i64(k::IDS)?;
    let contact_ids = r.required_i64(key::CONTACT_IDS)?;
    let checksums = r.optional_i64(key::CHECKSUMS)?;
    let types = r.optional_i32(k::TYPE)?;
    let labels = r.optional_strings(k::LABEL)?;
    let display_dates = r.optional_strings(k::DISPLAY_DATE)?;
    let actions = r.optional_i32(key::ACTIONS)?;
    let n = r.entries(batch_key)?;

    let mut batch = Batch::default();
    for i in 0..n {
        batch.entry(ids[i], action(actions, i), || {
            Ok(ContactDateRecord {
                id: ids[i],
                contact_id: contact_ids[i],
                checksum: long(checksums, i),
                kind: int(types, i),
                label: text(labels, i),
                display_date: text(display_dates, i),
            })
        })?;
    }
    Ok(Some(batch))
}

fn parse_contact_urls(item: &ValueSet) -> Result<Option<Batch<ContactUrlRecord>>, Malformed> {
    use url_key as k;
    let batch_key = RecordType::ContactUrl.batch_key();
    let Some(mut r) = BatchReader::open(item, batch_key)? else {
        return Ok(None);
    };

    let ids = r.required_i64(k::IDS)?;
    let contact_ids = r.required_i64(key::CONTACT_IDS)?;
    let checksums = r.optional_i64(key::CHECKSUMS)?;
    let types = r.optional_i32(k::TYPE)?;
    let labels = r.optional_strings(k::LABEL)?;
    let addresses = r.optional_strings(k::ADDRESS)?;
    let actions = r.optional_i32(key::ACTIONS)?;
    let n = r.entries(batch_key)?;

    let mut batch = Batch::default();
    for i in 0..n {
        batch.entry(ids[i], action(actions, i), || {
            Ok(ContactUrlRecord {
                id: ids[i],
                contact_id: contact_ids[i],
                checksum: long(checksums, i),
                kind: int(types, i),
                label: text(labels, i),
                address: text(addresses, i),
            })
        })?;
    }
    Ok(Some(batch))
}

#[cfg(test)]
mod tests;
