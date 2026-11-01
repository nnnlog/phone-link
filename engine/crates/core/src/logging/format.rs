//! One log line per event: `<time> <LEVEL> <target>[:<span>…]: <message> <field>=<value> …`, with
//! each field written as [`policy::class_of`] says.
//!
//! The message is written as it is for the engine's own events, whose messages are constants
//! (`scripts/log_calls.py` checks it) or lines this module rendered already; a dependency's message
//! is free text and is scrubbed.

use std::fmt::{self, Write as _};
use std::sync::Arc;

use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::fmt::format::Writer;
use tracing_subscriber::fmt::time::{FormatTime, SystemTime};
use tracing_subscriber::fmt::{FmtContext, FormatEvent, FormatFields};
use tracing_subscriber::registry::LookupSpan;

use super::policy::{self, FieldClass};
use super::redact::Redactor;

/// The prefix of the engine's crate names: `phonelink_engine` and the `phonelink_*` crates beneath it.
const OWN_CRATE_PREFIX: &str = "phonelink_";

/// The [`FormatEvent`] of the app's log.
pub(crate) struct LineFormat {
    redactor: Arc<Redactor>,
}

impl LineFormat {
    pub(crate) fn new(redactor: Arc<Redactor>) -> Self {
        Self { redactor }
    }
}

impl<S, N> FormatEvent<S, N> for LineFormat
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(&self, ctx: &FmtContext<'_, S, N>, mut writer: Writer<'_>, event: &Event<'_>) -> fmt::Result {
        let meta = event.metadata();
        SystemTime.format_time(&mut writer)?;
        write!(writer, " {:>5} {}", meta.level(), meta.target())?;

        // Span names are constants; their fields are not written.
        if let Some(scope) = ctx.event_scope() {
            for span in scope.from_root() {
                write!(writer, ":{}", span.name())?;
            }
        }

        let own = meta.module_path().is_some_and(|m| m.starts_with(OWN_CRATE_PREFIX));
        let mut line =
            Line { redactor: &self.redactor, own_message: own, message: String::new(), fields: String::new() };
        event.record(&mut line);
        writeln!(writer, ": {}{}", line.message, line.fields)
    }
}

/// The message and fields of one event, as they will be written.
struct Line<'a> {
    redactor: &'a Redactor,
    own_message: bool,
    message: String,
    fields: String,
}

impl Line<'_> {
    /// Adds field `name`, whose value reads `text`; `quoted` is `text` as a string's `Debug`, which
    /// is what a public string field shows.
    fn add(&mut self, name: &str, text: &str, quoted: Option<String>) {
        if name == "message" {
            self.message = if self.own_message { text.to_owned() } else { self.redactor.scrub(text) };
            return;
        }

        // The `log` crate's records carry their origin as `log.*` fields; the target already shows it.
        if name.starts_with("log.") {
            return;
        }

        let value = match policy::class_of(name) {
            FieldClass::Public => quoted.unwrap_or_else(|| text.to_owned()),
            FieldClass::FreeText => self.redactor.scrub(text),
            FieldClass::Redacted => self.redactor.token(name.rsplit('.').next().unwrap_or(name), text),
        };

        // Writing to a String does not fail.
        let _ = write!(self.fields, " {name}={value}");
    }
}

impl Visit for Line<'_> {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.add(field.name(), value, Some(format!("{value:?}")));
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        let text = format!("{value:?}");
        // A `?` string field arrives quoted; its tag is computed over the string itself, as for `%`.
        let bare = text.strip_prefix('"').and_then(|t| t.strip_suffix('"'));
        match bare {
            Some(bare) if policy::class_of(field.name()) == FieldClass::Redacted => self.add(field.name(), bare, None),
            _ => self.add(field.name(), &text, None),
        }
    }
}
