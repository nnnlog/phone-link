//! The engine's log, read by tests that wait for something that leaves no trace on the wire: a change
//! or a connect that starts nothing, a task that stops. Each such step writes its line once it has run.
//!
//! Two recorders. [`install`] sets a global `tracing` subscriber for the whole test binary that keeps
//! the CDEH identity's session changes and the App layer's connects, and a test reads the lines of
//! its own phone while other tests run beside it ([`changes`], [`connects`]). [`capture_thread`]
//! keeps every engine line written on the current thread, which on a current-thread runtime is every
//! line of the test's own engine.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Arc, Mutex, Once};

use phonelink_engine::types::DcgClient;
use tracing::field::{Field, Visit};
use tracing::subscriber::DefaultGuard;
use tracing::{Event, Subscriber};
use tracing_subscriber::Layer;
use tracing_subscriber::filter::filter_fn;
use tracing_subscriber::layer::{Context, SubscriberExt};

/// The modules [`install`] keeps: the CDEH identity's session changes, the App layer's connects.
const TARGETS: [&str; 2] = ["phonelink_engine::engine::cdeh", "phonelink_app::connect"];

static LINES: Mutex<Vec<Line>> = Mutex::new(Vec::new());

/// One event: its message and its fields, each as it would be written.
#[derive(Debug, Clone)]
struct Line {
    message: String,
    fields: BTreeMap<String, String>,
}

/// Where a recorder keeps its lines.
enum Sink {
    Global,
    Thread(Arc<Mutex<Vec<Line>>>),
}

struct Recorder(Sink);

impl<S: Subscriber> Layer<S> for Recorder {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let mut line = Line { message: String::new(), fields: BTreeMap::new() };
        event.record(&mut line);
        match &self.0 {
            Sink::Global => LINES.lock().unwrap().push(line),
            Sink::Thread(lines) => lines.lock().unwrap().push(line),
        }
    }
}

impl Visit for Line {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        let text = format!("{value:?}");
        if field.name() == "message" {
            self.message = text;
        } else {
            self.fields.insert(field.name().to_string(), text);
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.fields.insert(field.name().to_string(), value.to_string());
    }
}

/// Installs the global recorder once for the test binary; call it before the engine is built.
pub fn install() {
    static INSTALLED: Once = Once::new();
    INSTALLED.call_once(|| {
        let recorder = Recorder(Sink::Global).with_filter(filter_fn(|metadata| TARGETS.contains(&metadata.target())));
        tracing::subscriber::set_global_default(tracing_subscriber::registry().with(recorder))
            .expect("no other global subscriber in the test binary");
    });
}

fn lines_of(phone: &DcgClient) -> Vec<Line> {
    let phone = phone.to_string();
    LINES.lock().unwrap().iter().filter(|line| line.fields.get("phone") == Some(&phone)).cloned().collect()
}

/// The CDEH session changes for `phone` so far, one per change, in the order they arrived:
/// `starting <ypTriggerId>`, `not started: <reason>`, `stopping`, or `superseded <start
/// <ypTriggerId>|skip|stop>` for one a later change followed into the queue before it ran.
pub fn changes(phone: &DcgClient) -> Vec<String> {
    lines_of(phone)
        .iter()
        .filter_map(|line| match line.message.as_str() {
            "the identity's session starting" => Some(format!("starting {}", line.fields["trigger"])),
            "the identity's session not started" => Some(format!("not started: {}", line.fields["reason"])),
            "the identity's session stopping" => Some("stopping".to_string()),
            "session change superseded by a later one; skipped" => Some(match line.fields.get("trigger") {
                Some(trigger) => format!("superseded {} {trigger}", line.fields["change_kind"]),
                None => format!("superseded {}", line.fields["change_kind"]),
            }),
            _ => None,
        })
        .collect()
}

/// The connects the App layer's triggers sent to `phone` so far: `<trigger> triggered` when the
/// trigger fired, then, once the connect ended, `<trigger> completed`, `<trigger> dropped` (another
/// background connect was in flight), or `<trigger> not sent`.
pub fn connects(phone: &DcgClient) -> Vec<String> {
    lines_of(phone)
        .iter()
        .filter_map(|line| {
            let what = match line.message.as_str() {
                "connect triggered" => "triggered",
                "connect completed" => "completed",
                "connect not sent; a background connect is in flight" => "dropped",
                "connect not sent" => "not sent",
                _ => return None,
            };
            Some(format!("{} {what}", line.fields["trigger"]))
        })
        .collect()
}

/// How many connects `trigger` triggered to `phone`, once each of them has ended; `None` while one
/// is still running.
pub fn connects_ended(phone: &DcgClient, trigger: &str) -> Option<usize> {
    let lines = connects(phone);
    let of = |what: &str| lines.iter().filter(|l| l.strip_prefix(trigger) == Some(what)).count();
    let triggered = of(" triggered");
    (of(" completed") + of(" dropped") + of(" not sent") == triggered).then_some(triggered)
}

/// The engine lines written on this thread while it lives.
pub struct ThreadCapture {
    lines: Arc<Mutex<Vec<Line>>>,
    _guard: DefaultGuard,
}

/// Keeps every engine line written on the current thread from now on.
pub fn capture_thread() -> ThreadCapture {
    let lines = Arc::new(Mutex::new(Vec::new()));
    let recorder = Recorder(Sink::Thread(lines.clone()))
        .with_filter(filter_fn(|metadata| metadata.target().starts_with("phonelink_")));
    let guard = tracing::subscriber::set_default(tracing_subscriber::registry().with(recorder));
    ThreadCapture { lines, _guard: guard }
}

impl ThreadCapture {
    /// Whether a line with `message` was written.
    pub fn logged(&self, message: &str) -> bool {
        self.count(message) > 0
    }

    /// How many lines with `message` were written.
    pub fn count(&self, message: &str) -> usize {
        self.lines.lock().unwrap().iter().filter(|line| line.message == message).count()
    }
}
