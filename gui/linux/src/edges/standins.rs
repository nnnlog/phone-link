//! A system notifier that writes the notifications to standard error, for a session without a
//! notification service.

use phonelink_engine::edge::EdgeError;
use phonelink_engine::engine::{SystemNotification, SystemNotifier};

/// Writes each notification and withdrawal as a line on standard error.
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
