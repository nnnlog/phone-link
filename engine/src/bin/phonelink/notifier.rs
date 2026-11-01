//! The CLI's stand-in for the system-notification OS edge: posts and withdrawals are printed to
//! stderr (titles and bodies are phone content — the CLI runs only on the user's own machine).

use phonelink_engine::edge::EdgeError;
use phonelink_engine::engine::{SystemNotification, SystemNotifier};

pub struct StderrNotifier;

impl SystemNotifier for StderrNotifier {
    fn post(&self, n: SystemNotification) -> Result<(), EdgeError> {
        let mode = if n.alert { "alert" } else { "silent" };
        let subtitle = n.subtitle.as_deref().map(|s| format!(" — {s}")).unwrap_or_default();
        eprintln!("[notify {mode}] {}: {}{} | {}", n.id, n.title, subtitle, n.body);
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
