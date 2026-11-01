//! The phone apps' OS edge: what the feature asks of the Mac — whether an installed application opens
//! a URI (macOS: Launch Services), and the order of app names in the user's language.
//!
//! The CLI uses [`PlainAppsHost`]; the tests use [`testing`].

/// The Mac's applications and language, for the phone apps.
#[uniffi::export(with_foreign)]
pub trait PhoneAppsHost: Send + Sync {
    /// An application installed on this Mac opens `uri`. Called for a recent entry's document URI
    /// while it is stored, on a thread that may block.
    fn can_open_uri(&self, uri: String) -> bool;
    /// The positions of `names` in the order the Mac's current locale sorts them, ignoring case,
    /// diacritics, kana type and width; equal names keep their order in `names`. The result holds
    /// every position of `names` exactly once.
    fn name_order(&self, names: Vec<String>) -> Vec<u32>;
}

/// The CLI's host: no application opens anything, and names sort by their lower-case form.
#[derive(Debug, Default)]
pub struct PlainAppsHost;

/// `names`' positions ordered by their lower-case form, equal names in their given order.
pub fn plain_order(names: &[String]) -> Vec<u32> {
    let keys: Vec<String> = names.iter().map(|n| n.to_lowercase()).collect();
    let mut order: Vec<u32> = (0..names.len()).map(|i| i as u32).collect();
    order.sort_by(|a, b| keys[*a as usize].cmp(&keys[*b as usize]));
    order
}

impl PhoneAppsHost for PlainAppsHost {
    fn can_open_uri(&self, _uri: String) -> bool {
        false
    }
    fn name_order(&self, names: Vec<String>) -> Vec<u32> {
        plain_order(&names)
    }
}

/// In-memory stand-ins for tests.
#[cfg(any(test, feature = "test-support"))]
pub mod testing {
    use phonelink_core::sync::MutexExt;
    use std::collections::HashSet;
    use std::sync::{Arc, Mutex};

    use super::{PhoneAppsHost, plain_order};

    /// Opens the URIs it was given; records every query; sorts like [`PlainAppsHost`] unless an order
    /// is forced.
    #[derive(Default)]
    pub struct RecordingAppsHost {
        pub openable: Mutex<HashSet<String>>,
        pub queries: Mutex<Vec<String>>,
        /// Returned by `name_order` instead of the plain order (to test a broken edge).
        pub forced_order: Mutex<Option<Vec<u32>>>,
    }

    impl RecordingAppsHost {
        pub fn new() -> Arc<Self> {
            Arc::new(Self::default())
        }

        pub fn opening(uris: &[&str]) -> Arc<Self> {
            let host = Self::new();
            host.openable.lock_or_panic().extend(uris.iter().map(|u| u.to_string()));
            host
        }

        pub fn queries(&self) -> Vec<String> {
            self.queries.lock_or_panic().clone()
        }
    }

    impl PhoneAppsHost for RecordingAppsHost {
        fn can_open_uri(&self, uri: String) -> bool {
            self.queries.lock_or_panic().push(uri.clone());
            self.openable.lock_or_panic().contains(&uri)
        }
        fn name_order(&self, names: Vec<String>) -> Vec<u32> {
            match self.forced_order.lock_or_panic().clone() {
                Some(order) => order,
                None => plain_order(&names),
            }
        }
    }
}
