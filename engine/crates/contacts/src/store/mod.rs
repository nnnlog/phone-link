//! The contacts store of one phone — the app's own persistent store, a per-phone `contacts.sqlite3`,
//! holding exactly the local state the protocol depends on.
//!
//! It covers the six record types keyed by their `Int64` ids with checksums (contacts with their
//! thumbnail bytes, phone numbers with their display number, `NULL` when empty, and owning contact
//! id), the *current* and *expected* sequence numbers, and the PC consent, "contacts set up once"
//! and last-updated time.
//!
//! The contacts device permission
//! and `ContactsCapabilities` live in the App layer's phone record.
//!
//! Each change a message makes is one transaction (reconciliation, each batch, completion).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use rusqlite::{OptionalExtension, Transaction, params};

use phonelink_features::shared::db::{FeatureDb, StoreError};

use super::model::ContactNumber;
use super::parse::{
    Batch, ContactDateRecord, ContactRecord, ContactUrlRecord, EmailAddressRecord, ItemMessage, MetadataMessage,
    PhoneNumberRecord, PostalAddressRecord, SequenceNumbers,
};
use super::view::{ListedContact, shown_name};
use super::wire::RecordType;

/// The store's feature name (`<root>/phones/<phone>/contacts.sqlite3`).
pub const FEATURE: &str = "contacts";

/// The schema, one migration per entry.
pub const MIGRATIONS: &[&str] = &["
    CREATE TABLE contact (
        id INTEGER PRIMARY KEY,
        checksum INTEGER NOT NULL,
        last_updated_ms INTEGER NOT NULL,
        display_name TEXT NOT NULL,
        nickname TEXT NOT NULL,
        company TEXT NOT NULL,
        job_title TEXT NOT NULL,
        notes TEXT NOT NULL,
        name_prefix TEXT NOT NULL,
        name_suffix TEXT NOT NULL,
        given_name TEXT NOT NULL,
        middle_name TEXT NOT NULL,
        family_name TEXT NOT NULL,
        thumbnail BLOB
    );
    CREATE TABLE phone_number (
        id INTEGER PRIMARY KEY,
        contact_id INTEGER NOT NULL,
        checksum INTEGER NOT NULL,
        number TEXT NOT NULL,
        display_number TEXT,
        type INTEGER NOT NULL,
        label TEXT
    );
    CREATE INDEX phone_number_contact ON phone_number (contact_id);
    CREATE TABLE email_address (
        id INTEGER PRIMARY KEY,
        contact_id INTEGER NOT NULL,
        checksum INTEGER NOT NULL,
        address TEXT NOT NULL,
        type INTEGER NOT NULL,
        label TEXT NOT NULL
    );
    CREATE INDEX email_address_contact ON email_address (contact_id);
    CREATE TABLE postal_address (
        id INTEGER PRIMARY KEY,
        contact_id INTEGER NOT NULL,
        checksum INTEGER NOT NULL,
        type INTEGER NOT NULL,
        label TEXT NOT NULL,
        display_address TEXT NOT NULL,
        street TEXT NOT NULL,
        city TEXT NOT NULL,
        region TEXT NOT NULL,
        postal_code TEXT NOT NULL,
        country_code TEXT NOT NULL,
        sub_administrative_area TEXT NOT NULL,
        sub_locality TEXT NOT NULL
    );
    CREATE INDEX postal_address_contact ON postal_address (contact_id);
    CREATE TABLE contact_date (
        id INTEGER PRIMARY KEY,
        contact_id INTEGER NOT NULL,
        checksum INTEGER NOT NULL,
        type INTEGER NOT NULL,
        label TEXT NOT NULL,
        display_date TEXT NOT NULL
    );
    CREATE INDEX contact_date_contact ON contact_date (contact_id);
    CREATE TABLE contact_url (
        id INTEGER PRIMARY KEY,
        contact_id INTEGER NOT NULL,
        checksum INTEGER NOT NULL,
        type INTEGER NOT NULL,
        label TEXT NOT NULL,
        address TEXT NOT NULL
    );
    CREATE INDEX contact_url_contact ON contact_url (contact_id);
    CREATE TABLE sequence_number (
        record_type TEXT NOT NULL PRIMARY KEY,
        current INTEGER NOT NULL DEFAULT -1,
        expected INTEGER NOT NULL DEFAULT -1
    );
    CREATE TABLE state (
        id INTEGER NOT NULL PRIMARY KEY CHECK (id = 0),
        pc_consent INTEGER NOT NULL DEFAULT 1,
        setup_completed_once INTEGER NOT NULL DEFAULT 0,
        last_updated_ms INTEGER
    );
    INSERT INTO state (id) VALUES (0);
"];

/// A stored type's *current* and *expected* sequence numbers; a type without a row holds `-1` and
/// takes no part in the up-to-date test.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StoredSequence {
    pub current: i64,
    pub expected: i64,
}

/// A contact as a lookup reads it ([`ContactsStore::contact_summary`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContactSummary {
    pub id: i64,
    pub display_name: String,
    pub has_thumbnail: bool,
}

/// The per-phone flags of the local state the protocol depends on, kept in this store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StoredState {
    /// PC consent (default `true`).
    pub pc_consent: bool,
    /// "Contacts set up once" (default `false`).
    pub setup_completed_once: bool,
    /// When the last sync session completed (Unix ms); `None` = never.
    pub last_updated_ms: Option<i64>,
}

/// A phone number or e-mail address with its owning contact, for the lookup index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddressRow {
    pub address: String,
    pub contact_id: i64,
}

/// The contacts store of one phone.
#[derive(Debug, Clone)]
pub struct ContactsStore {
    db: Arc<FeatureDb>,
}

impl ContactsStore {
    pub fn new(db: Arc<FeatureDb>) -> Self {
        Self { db }
    }

    /// The reconciliation of a metadata message, in one transaction: the six types in order, each
    /// deleting by omission and requesting what is missing or changed (contacts also those stored
    /// without a thumbnail and the owners of display-less phone numbers). When nothing is requested
    /// the session completes in the same transaction (`completion`). Returns the requested ids per
    /// type.
    pub fn apply_metadata(
        &self,
        msg: &MetadataMessage,
        completion: impl FnOnce(&[Vec<i64>; 6]) -> Option<Completion>,
    ) -> Result<[Vec<i64>; 6], StoreError> {
        self.db.transaction(|tx| {
            let requested = reconcile(tx, msg)?;
            if let Some(c) = completion(&requested) {
                complete(tx, &c)?;
            }
            Ok::<_, StoreError>(requested)
        })
    }

    /// Applies an item message's batches (each type's deletes, then its upserts) and, when the
    /// session completes with it, the completion — in one transaction.
    pub fn apply_item(&self, item: &ItemMessage, completion: Option<Completion>) -> Result<(), StoreError> {
        self.db.transaction(|tx| {
            if let Some(b) = &item.contacts {
                apply_batch(tx, RecordType::Contact, b, upsert_contact)?;
            }
            if let Some(b) = &item.phone_numbers {
                apply_batch(tx, RecordType::PhoneNumber, b, upsert_phone_number)?;
            }
            if let Some(b) = &item.email_addresses {
                apply_batch(tx, RecordType::EmailAddress, b, upsert_email_address)?;
            }
            if let Some(b) = &item.postal_addresses {
                apply_batch(tx, RecordType::PostalAddress, b, upsert_postal_address)?;
            }
            if let Some(b) = &item.contact_dates {
                apply_batch(tx, RecordType::ContactDate, b, upsert_contact_date)?;
            }
            if let Some(b) = &item.contact_urls {
                apply_batch(tx, RecordType::ContactUrl, b, upsert_contact_url)?;
            }

            if let Some(c) = &completion {
                complete(tx, c)?;
            }
            Ok::<_, StoreError>(())
        })
    }

    /// The stored sequence numbers by type (`None` = no row: `-1` / `-1`).
    pub fn sequence_numbers(&self) -> Result<[Option<StoredSequence>; 6], StoreError> {
        self.db.read(|c| {
            let mut out = [None; 6];
            let mut stmt = c.prepare("SELECT record_type, current, expected FROM sequence_number")?;
            let rows =
                stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?)))?;

            for row in rows {
                let (name, current, expected) = row?;
                if let Some(t) = RecordType::ALL.iter().find(|t| t.sequence_key() == name) {
                    out[t.index()] = Some(StoredSequence { current, expected });
                }
            }
            Ok(out)
        })
    }

    /// Writes all six *expected* sequence numbers (a connect response).
    pub fn set_expected(&self, expected: [i64; 6]) -> Result<(), StoreError> {
        self.db.transaction(|tx| {
            let mut stmt = tx.prepare(
                "INSERT INTO sequence_number (record_type, expected) VALUES (?1, ?2)
                 ON CONFLICT (record_type) DO UPDATE SET expected = excluded.expected",
            )?;
            for t in RecordType::ALL {
                stmt.execute(params![t.sequence_key(), expected[t.index()]])?;
            }
            Ok::<_, StoreError>(())
        })
    }

    pub fn state(&self) -> Result<StoredState, StoreError> {
        self.db.read(|c| {
            c.query_row("SELECT pc_consent, setup_completed_once, last_updated_ms FROM state WHERE id = 0", [], |r| {
                Ok(StoredState { pc_consent: r.get(0)?, setup_completed_once: r.get(1)?, last_updated_ms: r.get(2)? })
            })
        })
    }

    /// Sets PC consent; whether it changed.
    pub fn set_pc_consent(&self, consent: bool) -> Result<bool, StoreError> {
        self.db.transaction(|tx| {
            let n = tx.execute("UPDATE state SET pc_consent = ?1 WHERE id = 0 AND pc_consent != ?1", [consent])?;
            Ok::<_, StoreError>(n > 0)
        })
    }

    /// Sets "contacts set up once"; whether it changed.
    pub fn set_setup_completed_once(&self) -> Result<bool, StoreError> {
        self.db.transaction(|tx| {
            let n =
                tx.execute("UPDATE state SET setup_completed_once = 1 WHERE id = 0 AND setup_completed_once = 0", [])?;
            Ok::<_, StoreError>(n > 0)
        })
    }

    /// Deletes every record of the six types and resets every sequence number to `-1`, as a newly
    /// created store holds; the PC consent, "set up once" and last-updated time stay.
    pub fn clear(&self) -> Result<(), StoreError> {
        self.db.transaction(|tx| {
            for t in RecordType::ALL {
                tx.execute(&format!("DELETE FROM {}", t.table()), [])?;
            }
            tx.execute("DELETE FROM sequence_number", [])?;
            Ok::<_, StoreError>(())
        })
    }

    // --- views ------------------------------------------------------------------------------------

    /// `(id, display name, has thumbnail)` of every contact, by display name then id.
    pub fn contact_list(&self) -> Result<Vec<(i64, String, bool)>, StoreError> {
        self.db.read(|c| {
            let mut stmt = c.prepare(
                "SELECT id, display_name, thumbnail IS NOT NULL FROM contact
                 ORDER BY display_name COLLATE NOCASE, id",
            )?;
            let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
            rows.collect()
        })
    }

    /// Every phone number whose contact is stored, with the contact's name and whether it has a
    /// thumbnail (not its bytes): by display name (case-insensitively), contact id, then number id.
    pub fn contact_numbers(&self) -> Result<Vec<ContactNumber>, StoreError> {
        self.db.read(|c| {
            let mut stmt = c.prepare(
                "SELECT c.id, c.display_name, c.thumbnail IS NOT NULL, p.number, p.display_number, p.type, p.label
                 FROM phone_number p JOIN contact c ON c.id = p.contact_id
                 ORDER BY c.display_name COLLATE NOCASE, c.id, p.id",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok(ContactNumber {
                    contact_id: r.get(0)?,
                    display_name: r.get(1)?,
                    has_thumbnail: r.get(2)?,
                    number: r.get(3)?,
                    display_number: r.get::<_, Option<String>>(4)?.unwrap_or_default(),
                    kind: r.get(5)?,
                    label: r.get::<_, Option<String>>(6)?.unwrap_or_default(),
                })
            })?;
            rows.collect()
        })
    }

    /// Every contact as the contacts view lists it, by display name (case-insensitively) then id,
    /// with the text of its fields and sub-records that the view's search reads.
    pub fn listed_contacts(&self) -> Result<Vec<ListedContact>, StoreError> {
        self.db.read(|c| {
            let mut terms: HashMap<i64, Vec<String>> = HashMap::new();
            let mut stmt = c.prepare(
                "SELECT contact_id, number FROM phone_number
                 UNION ALL SELECT contact_id, display_number FROM phone_number WHERE display_number IS NOT NULL
                 UNION ALL SELECT contact_id, address FROM email_address
                 UNION ALL SELECT contact_id, display_address FROM postal_address
                 UNION ALL SELECT contact_id, address FROM contact_url
                 UNION ALL SELECT contact_id, display_date FROM contact_date",
            )?;
            for row in stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))? {
                let (id, text) = row?;
                terms.entry(id).or_default().push(text);
            }

            let mut stmt = c.prepare(
                "SELECT id, display_name, nickname, company, job_title, notes, thumbnail IS NOT NULL FROM contact
                 ORDER BY display_name COLLATE NOCASE, id",
            )?;
            let rows = stmt.query_map([], |r| {
                let id: i64 = r.get(0)?;
                let display_name: String = r.get(1)?;
                let nickname: String = r.get(2)?;
                let company: String = r.get(3)?;

                let mut own = vec![nickname.clone(), company.clone(), r.get(4)?, r.get(5)?];
                own.extend(terms.remove(&id).unwrap_or_default());
                own.retain(|t| !t.is_empty());

                Ok(ListedContact {
                    id,
                    name: shown_name(&display_name, &company, &nickname).to_owned(),
                    has_thumbnail: r.get(6)?,
                    terms: own,
                })
            })?;
            rows.collect()
        })
    }

    /// A contact's id, display name and whether a thumbnail is stored — what a lookup needs —
    /// without its other fields or the thumbnail's bytes.
    pub fn contact_summary(&self, id: i64) -> Result<Option<ContactSummary>, StoreError> {
        self.db.read(|c| {
            c.query_row("SELECT id, display_name, thumbnail IS NOT NULL FROM contact WHERE id = ?1", [id], |r| {
                Ok(ContactSummary { id: r.get(0)?, display_name: r.get(1)?, has_thumbnail: r.get(2)? })
            })
            .optional()
        })
    }

    pub fn contact(&self, id: i64) -> Result<Option<ContactRecord>, StoreError> {
        self.db.read(|c| {
            c.query_row(
                "SELECT id, checksum, last_updated_ms, display_name, nickname, company, job_title, notes,
                        name_prefix, name_suffix, given_name, middle_name, family_name, thumbnail
                 FROM contact WHERE id = ?1",
                [id],
                |r| {
                    Ok(ContactRecord {
                        id: r.get(0)?,
                        checksum: r.get(1)?,
                        last_updated_ms: r.get(2)?,
                        display_name: r.get(3)?,
                        nickname: r.get(4)?,
                        company: r.get(5)?,
                        job_title: r.get(6)?,
                        notes: r.get(7)?,
                        name_prefix: r.get(8)?,
                        name_suffix: r.get(9)?,
                        given_name: r.get(10)?,
                        middle_name: r.get(11)?,
                        family_name: r.get(12)?,
                        thumbnail: r.get(13)?,
                    })
                },
            )
            .optional()
        })
    }

    pub fn thumbnail(&self, id: i64) -> Result<Option<Vec<u8>>, StoreError> {
        self.db.read(|c| {
            c.query_row("SELECT thumbnail FROM contact WHERE id = ?1", [id], |r| r.get::<_, Option<Vec<u8>>>(0))
                .optional()
                .map(Option::flatten)
        })
    }

    /// The phone numbers stored for `contact_id` (also after the contact itself was deleted), by id.
    pub fn phone_numbers(&self, contact_id: i64) -> Result<Vec<PhoneNumberRecord>, StoreError> {
        self.db.read(|c| {
            let mut stmt = c.prepare(
                "SELECT id, contact_id, checksum, number, display_number, type, label
                 FROM phone_number WHERE contact_id = ?1 ORDER BY id",
            )?;
            let rows = stmt.query_map([contact_id], |r| {
                Ok(PhoneNumberRecord {
                    id: r.get(0)?,
                    contact_id: r.get(1)?,
                    checksum: r.get(2)?,
                    number: r.get(3)?,
                    display_number: r.get::<_, Option<String>>(4)?.unwrap_or_default(),
                    kind: r.get(5)?,
                    label: r.get::<_, Option<String>>(6)?.unwrap_or_default(),
                })
            })?;
            rows.collect()
        })
    }

    pub fn email_addresses(&self, contact_id: i64) -> Result<Vec<EmailAddressRecord>, StoreError> {
        self.db.read(|c| {
            let mut stmt = c.prepare(
                "SELECT id, contact_id, checksum, address, type, label FROM email_address
                 WHERE contact_id = ?1 ORDER BY id",
            )?;
            let rows = stmt.query_map([contact_id], |r| {
                Ok(EmailAddressRecord {
                    id: r.get(0)?,
                    contact_id: r.get(1)?,
                    checksum: r.get(2)?,
                    address: r.get(3)?,
                    kind: r.get(4)?,
                    label: r.get(5)?,
                })
            })?;
            rows.collect()
        })
    }

    pub fn postal_addresses(&self, contact_id: i64) -> Result<Vec<PostalAddressRecord>, StoreError> {
        self.db.read(|c| {
            let mut stmt = c.prepare(
                "SELECT id, contact_id, checksum, type, label, display_address, street, city, region, postal_code,
                        country_code, sub_administrative_area, sub_locality
                 FROM postal_address WHERE contact_id = ?1 ORDER BY id",
            )?;
            let rows = stmt.query_map([contact_id], |r| {
                Ok(PostalAddressRecord {
                    id: r.get(0)?,
                    contact_id: r.get(1)?,
                    checksum: r.get(2)?,
                    kind: r.get(3)?,
                    label: r.get(4)?,
                    display_address: r.get(5)?,
                    street: r.get(6)?,
                    city: r.get(7)?,
                    region: r.get(8)?,
                    postal_code: r.get(9)?,
                    country_code: r.get(10)?,
                    sub_administrative_area: r.get(11)?,
                    sub_locality: r.get(12)?,
                })
            })?;
            rows.collect()
        })
    }

    pub fn contact_dates(&self, contact_id: i64) -> Result<Vec<ContactDateRecord>, StoreError> {
        self.db.read(|c| {
            let mut stmt = c.prepare(
                "SELECT id, contact_id, checksum, type, label, display_date FROM contact_date
                 WHERE contact_id = ?1 ORDER BY id",
            )?;
            let rows = stmt.query_map([contact_id], |r| {
                Ok(ContactDateRecord {
                    id: r.get(0)?,
                    contact_id: r.get(1)?,
                    checksum: r.get(2)?,
                    kind: r.get(3)?,
                    label: r.get(4)?,
                    display_date: r.get(5)?,
                })
            })?;
            rows.collect()
        })
    }

    pub fn contact_urls(&self, contact_id: i64) -> Result<Vec<ContactUrlRecord>, StoreError> {
        self.db.read(|c| {
            let mut stmt = c.prepare(
                "SELECT id, contact_id, checksum, type, label, address FROM contact_url
                 WHERE contact_id = ?1 ORDER BY id",
            )?;
            let rows = stmt.query_map([contact_id], |r| {
                Ok(ContactUrlRecord {
                    id: r.get(0)?,
                    contact_id: r.get(1)?,
                    checksum: r.get(2)?,
                    kind: r.get(3)?,
                    label: r.get(4)?,
                    address: r.get(5)?,
                })
            })?;
            rows.collect()
        })
    }

    #[cfg(feature = "test-support")]
    /// The stored records of `t`: `(id, checksum)` by id.
    pub fn records(&self, t: RecordType) -> Result<Vec<(i64, i64)>, StoreError> {
        self.db.read(|c| {
            let mut stmt = c.prepare(&format!("SELECT id, checksum FROM {} ORDER BY id", t.table()))?;
            let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect()
        })
    }

    /// Every stored `phoneNumbers` value whose owning contact is stored, by contact id then record id.
    pub fn phone_number_rows(&self) -> Result<Vec<AddressRow>, StoreError> {
        self.address_rows(
            "SELECT p.number, p.contact_id FROM phone_number p JOIN contact c ON c.id = p.contact_id
             ORDER BY p.contact_id, p.id",
        )
    }

    /// Every stored e-mail address whose owning contact is stored, by contact id then record id.
    pub fn email_rows(&self) -> Result<Vec<AddressRow>, StoreError> {
        self.address_rows(
            "SELECT e.address, e.contact_id FROM email_address e JOIN contact c ON c.id = e.contact_id
             ORDER BY e.contact_id, e.id",
        )
    }

    fn address_rows(&self, sql: &str) -> Result<Vec<AddressRow>, StoreError> {
        self.db.read(|c| {
            let mut stmt = c.prepare(sql)?;
            let rows = stmt.query_map([], |r| Ok(AddressRow { address: r.get(0)?, contact_id: r.get(1)? }))?;
            rows.collect()
        })
    }
}

/// What completion writes: the session's *current* sequence numbers (values of `-1` are not
/// written; a written value replaces the stored one even when lower) and the last-updated time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Completion {
    pub sequence_numbers: SequenceNumbers,
    pub now_ms: i64,
}

fn complete(tx: &Transaction<'_>, c: &Completion) -> rusqlite::Result<()> {
    let mut stmt = tx.prepare(
        "INSERT INTO sequence_number (record_type, current) VALUES (?1, ?2)
         ON CONFLICT (record_type) DO UPDATE SET current = excluded.current",
    )?;
    for t in RecordType::ALL {
        let v = c.sequence_numbers.get(t);
        if v != -1 {
            stmt.execute(params![t.sequence_key(), v])?;
        }
    }

    tx.execute("UPDATE state SET last_updated_ms = ?1 WHERE id = 0", [c.now_ms])?;
    Ok(())
}

/// The reconciliation of the six types in order: delete by omission, then request what is missing or
/// changed.
fn reconcile(tx: &Transaction<'_>, msg: &MetadataMessage) -> rusqlite::Result<[Vec<i64>; 6]> {
    let mut requested: [Vec<i64>; 6] = Default::default();
    for t in RecordType::ALL {
        let ids = &msg.ids[t.index()];
        let sums = &msg.checksums[t.index()];
        let contact = t == RecordType::Contact;

        // Stored records: id → (checksum, stored without a thumbnail).
        let stored: Vec<(i64, i64, bool)> = {
            let sql =
                format!("SELECT id, checksum, {} FROM {}", if contact { "thumbnail IS NULL" } else { "0" }, t.table());
            let mut stmt = tx.prepare(&sql)?;
            let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
            rows.collect::<rusqlite::Result<_>>()?
        };

        // 1. Delete by omission (an empty or absent id list deletes every record of the type).
        let incoming: HashSet<i64> = ids.iter().copied().collect();
        {
            let mut delete = tx.prepare(&format!("DELETE FROM {} WHERE id = ?1", t.table()))?;
            for (id, _, _) in stored.iter().filter(|(id, _, _)| !incoming.contains(id)) {
                delete.execute([id])?;
            }
        }

        // 2. Request what is missing or has another checksum (contacts: or no thumbnail).
        let kept: HashMap<i64, (i64, bool)> = stored
            .into_iter()
            .filter(|(id, _, _)| incoming.contains(id))
            .map(|(id, checksum, no_thumbnail)| (id, (checksum, no_thumbnail)))
            .collect();
        let mut list: Vec<i64> = ids
            .iter()
            .zip(sums)
            .filter(|(id, sum)| match kept.get(id) {
                None => true,
                Some((checksum, no_thumbnail)) => checksum != *sum || (contact && *no_thumbnail),
            })
            .map(|(id, _)| *id)
            .collect();

        // 3. Contacts only: the owners of stored display-less phone numbers, before the phone-number
        //    type is reconciled; no duplicates.
        if contact {
            let mut stmt = tx.prepare(
                "SELECT DISTINCT contact_id FROM phone_number WHERE display_number IS NULL ORDER BY contact_id",
            )?;
            let owners = stmt.query_map([], |r| r.get::<_, i64>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
            list.extend(owners);
            let mut seen = HashSet::new();
            list.retain(|id| seen.insert(*id));
        }
        requested[t.index()] = list;
    }
    Ok(requested)
}

fn apply_batch<R>(
    tx: &Transaction<'_>,
    t: RecordType,
    batch: &Batch<R>,
    upsert: fn(&Transaction<'_>, &R) -> rusqlite::Result<()>,
) -> rusqlite::Result<()> {
    if !batch.deletes.is_empty() {
        let mut delete = tx.prepare(&format!("DELETE FROM {} WHERE id = ?1", t.table()))?;
        for id in &batch.deletes {
            delete.execute([id])?;
        }
    }

    for r in &batch.upserts {
        upsert(tx, r)?;
    }
    Ok(())
}

/// Inserts or replaces a contact; an upsert without a thumbnail keeps the stored one.
fn upsert_contact(tx: &Transaction<'_>, r: &ContactRecord) -> rusqlite::Result<()> {
    tx.prepare_cached(
        "INSERT INTO contact (id, checksum, last_updated_ms, display_name, nickname, company, job_title, notes,
                              name_prefix, name_suffix, given_name, middle_name, family_name, thumbnail)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
         ON CONFLICT (id) DO UPDATE SET
            checksum = excluded.checksum, last_updated_ms = excluded.last_updated_ms,
            display_name = excluded.display_name, nickname = excluded.nickname, company = excluded.company,
            job_title = excluded.job_title, notes = excluded.notes, name_prefix = excluded.name_prefix,
            name_suffix = excluded.name_suffix, given_name = excluded.given_name,
            middle_name = excluded.middle_name, family_name = excluded.family_name,
            thumbnail = COALESCE(excluded.thumbnail, thumbnail)",
    )?
    .execute(params![
        r.id,
        r.checksum,
        r.last_updated_ms,
        r.display_name,
        r.nickname,
        r.company,
        r.job_title,
        r.notes,
        r.name_prefix,
        r.name_suffix,
        r.given_name,
        r.middle_name,
        r.family_name,
        r.thumbnail,
    ])?;
    Ok(())
}

/// Phone numbers compare before they write: an empty display number or label is stored as `NULL`,
/// and the stored record is replaced only when the checksum, owner, number or type differs, both
/// labels are non-empty and differ, or the display number is non-empty and differs from the stored
/// one (or none is stored) — SQL's comparison with `NULL`.
fn upsert_phone_number(tx: &Transaction<'_>, r: &PhoneNumberRecord) -> rusqlite::Result<()> {
    let display = Some(r.display_number.as_str()).filter(|s| !s.is_empty());
    let label = Some(r.label.as_str()).filter(|s| !s.is_empty());

    tx.prepare_cached(
        "INSERT INTO phone_number (id, contact_id, checksum, number, display_number, type, label)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT (id) DO UPDATE SET
            contact_id = excluded.contact_id, checksum = excluded.checksum, number = excluded.number,
            display_number = excluded.display_number, type = excluded.type, label = excluded.label
         WHERE checksum != excluded.checksum OR contact_id != excluded.contact_id OR number != excluded.number
            OR type != excluded.type OR label != excluded.label
            OR (display_number IS NULL AND excluded.display_number IS NOT NULL)
            OR display_number != excluded.display_number",
    )?
    .execute(params![r.id, r.contact_id, r.checksum, r.number, display, r.kind, label])?;
    Ok(())
}

fn upsert_email_address(tx: &Transaction<'_>, r: &EmailAddressRecord) -> rusqlite::Result<()> {
    tx.prepare_cached(
        "INSERT INTO email_address (id, contact_id, checksum, address, type, label) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT (id) DO UPDATE SET contact_id = excluded.contact_id, checksum = excluded.checksum,
            address = excluded.address, type = excluded.type, label = excluded.label",
    )?
    .execute(params![r.id, r.contact_id, r.checksum, r.address, r.kind, r.label])?;
    Ok(())
}

fn upsert_postal_address(tx: &Transaction<'_>, r: &PostalAddressRecord) -> rusqlite::Result<()> {
    tx.prepare_cached(
        "INSERT INTO postal_address (id, contact_id, checksum, type, label, display_address, street, city, region,
                                     postal_code, country_code, sub_administrative_area, sub_locality)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
         ON CONFLICT (id) DO UPDATE SET contact_id = excluded.contact_id, checksum = excluded.checksum,
            type = excluded.type, label = excluded.label, display_address = excluded.display_address,
            street = excluded.street, city = excluded.city, region = excluded.region,
            postal_code = excluded.postal_code, country_code = excluded.country_code,
            sub_administrative_area = excluded.sub_administrative_area, sub_locality = excluded.sub_locality",
    )?
    .execute(params![
        r.id,
        r.contact_id,
        r.checksum,
        r.kind,
        r.label,
        r.display_address,
        r.street,
        r.city,
        r.region,
        r.postal_code,
        r.country_code,
        r.sub_administrative_area,
        r.sub_locality,
    ])?;
    Ok(())
}

fn upsert_contact_date(tx: &Transaction<'_>, r: &ContactDateRecord) -> rusqlite::Result<()> {
    tx.prepare_cached(
        "INSERT INTO contact_date (id, contact_id, checksum, type, label, display_date) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT (id) DO UPDATE SET contact_id = excluded.contact_id, checksum = excluded.checksum,
            type = excluded.type, label = excluded.label, display_date = excluded.display_date",
    )?
    .execute(params![r.id, r.contact_id, r.checksum, r.kind, r.label, r.display_date])?;
    Ok(())
}

fn upsert_contact_url(tx: &Transaction<'_>, r: &ContactUrlRecord) -> rusqlite::Result<()> {
    tx.prepare_cached(
        "INSERT INTO contact_url (id, contact_id, checksum, type, label, address) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT (id) DO UPDATE SET contact_id = excluded.contact_id, checksum = excluded.checksum,
            type = excluded.type, label = excluded.label, address = excluded.address",
    )?
    .execute(params![r.id, r.contact_id, r.checksum, r.kind, r.label, r.address])?;
    Ok(())
}

#[cfg(test)]
mod tests;
