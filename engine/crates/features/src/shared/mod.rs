//! What the features share.
//!
//! It covers their stores, the system notifications and where responses go, authentication codes in
//! notification text, the account's peers, the per-phone switch, phone numbers, the once-logged recurring failures, the files permission, the phone-apps gates and the Nano
//! share channels.

pub mod auth_code;
pub mod db;
pub mod files_permission;
pub mod launch_gates;
pub mod nano_channels;
pub mod notifier;
pub mod ongoing;
pub mod peers;
pub mod phone_number;
pub mod phone_switch;
