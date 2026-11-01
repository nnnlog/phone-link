use std::fs;
use std::io::Write;
use std::path::Path;
use std::sync::Arc;

use rand::Rng;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt::MakeWriter;

use super::redact::Redactor;
use super::store::{LogStore, ROTATE_AT};
use super::{Level, Segment, emit, render, subscriber};

struct World {
    dir: tempfile::TempDir,
    store: LogStore,
    redactor: Arc<Redactor>,
}

impl World {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let store = LogStore::open(dir.path()).unwrap();
        let redactor = Arc::new(Redactor::open(dir.path()).unwrap());
        Self { dir, store, redactor }
    }

    /// Runs `f` with this world's subscriber as the thread's default and returns the log's text.
    fn log(&self, f: impl FnOnce()) -> String {
        let sub = subscriber(self.store.clone(), Arc::clone(&self.redactor), EnvFilter::new("trace")).finish();
        tracing::subscriber::with_default(sub, f);
        self.text()
    }

    fn text(&self) -> String {
        let out = self.dir.path().join("export.log");
        self.store.export(&out).unwrap();
        fs::read_to_string(out).unwrap()
    }
}

fn tags(text: &str, kind: &str) -> Vec<String> {
    let open = format!("{{{{REDACTED:{kind}|");
    text.match_indices(&open).map(|(i, _)| text[i + open.len()..i + open.len() + 8].to_owned()).collect()
}

#[test]
fn a_field_not_on_the_public_list_is_written_as_a_token() {
    let w = World::new();
    let phone = "9f1c2d3e-0000-4000-8000-123456789abc";
    let text = w.log(|| tracing::info!(%phone, partner = ?phone, "connected"));

    assert!(!text.contains(phone), "{text}");
    let (p, q) = (tags(&text, "phone"), tags(&text, "partner"));
    assert_eq!((p.len(), q.len()), (1, 1), "{text}");
    assert_eq!(p, q, "a `%` and a `?` string field of one value get one tag");
}

#[test]
fn public_fields_and_the_message_are_written_as_they_are() {
    let w = World::new();
    let text = w.log(|| tracing::warn!(attempt = 3, route = "/legacy/phonecontent", ok = false, "App request failed"));
    assert!(text.contains(" WARN "), "{text}");
    assert!(text.contains(": App request failed attempt=3 route=\"/legacy/phonecontent\" ok=false"), "{text}");
}

#[test]
fn error_texts_are_scrubbed_of_identifier_shapes() {
    let w = World::new();
    let error = "negotiate https://dcg.example/relayhub?id=abc: no trust key 0123456789abcdef0123 for a@b.example \
                 at /Users/someone/Library/x.db, call +1 555 123 4567, \
                 entry {9F1C2D3E-0000-4000-8000-123456789ABC}, DeviceResourceManagerResponseMessage";

    let text = w.log(|| tracing::error!(%error, "failed"));

    for raw in ["dcg.example", "0123456789abcdef0123", "a@b.example", "someone", "555 123 4567", "9F1C2D3E"] {
        assert!(!text.contains(raw), "{raw} in {text}");
    }
    for kind in ["url", "id", "email", "path", "number"] {
        assert!(!tags(&text, kind).is_empty(), "no {kind} token in {text}");
    }
    assert!(text.contains("DeviceResourceManagerResponseMessage"), "a long word is not an identifier: {text}");
}

#[test]
fn values_logged_under_unlisted_names_never_reach_the_file() {
    let w = World::new();
    let mut rng = rand::rng();
    let values: Vec<String> = (0..200)
        .map(|_| (0..rng.random_range(4..40)).map(|_| rng.random_range(b'!'..=b'~') as char).collect())
        .collect();

    let text = w.log(|| {
        for v in &values {
            tracing::info!(name = %v, value = ?v, peer = v.as_str(), "item");
        }
    });

    for v in &values {
        // A value short enough to occur by chance inside a tag or the fixed text proves nothing.
        if v.len() >= 12 {
            assert!(!text.contains(v.as_str()), "{v:?} leaked");
        }
    }
}

#[test]
fn the_apps_lines_carry_their_private_parts_as_tokens() {
    let w = World::new();
    let segments = [
        Segment { text: "photo ", public: true },
        Segment { text: "IMG_0042.jpg", public: false },
        Segment { text: " has no image", public: true },
    ];

    let line = render(&w.redactor, &segments);
    let text = w.log(|| emit(Level::Notice, "photos", &line));

    assert!(!text.contains("IMG_0042"), "{text}");
    assert!(text.contains(" WARN gui: photo {{REDACTED:private|"), "{text}");
    assert!(text.contains("}} has no image category=\"photos\""), "{text}");
}

#[test]
fn the_apps_public_parts_are_scrubbed_of_paths() {
    let w = World::new();
    let error = "Error Domain=NSCocoaErrorDomain Code=4 NSFilePath=/Users/someone/Pictures/IMG_0042.jpg";
    let line = render(
        &w.redactor,
        &[Segment { text: "photo not removed: ", public: true }, Segment { text: error, public: true }],
    );

    assert!(!line.contains("someone"), "{line}");
    assert!(
        line.starts_with("photo not removed: Error Domain=NSCocoaErrorDomain Code=4 NSFilePath={{REDACTED:path|"),
        "{line}"
    );
}

#[test]
fn names_shaped_as_counts_enums_and_flags_are_public() {
    use super::policy::{FieldClass, class_of};
    for name in
        ["file_count", "frame_len", "elapsed_ms", "trust_type", "drag_phase", "is_secure", "ctx.has_key", "channel"]
    {
        assert_eq!(class_of(name), FieldClass::Public, "{name}");
    }

    for name in ["phone", "channel_id", "media_id", "error", "raw_content_type", "raw_status"] {
        assert_ne!(class_of(name), FieldClass::Public, "{name}");
    }

    assert_eq!(class_of("raw_content_type"), FieldClass::FreeText, "a raw value is scrubbed whatever its suffix");
}

#[test]
fn equal_values_share_a_tag_until_the_key_changes() {
    let w = World::new();
    let before = w.redactor.token("phone", "x");
    assert_eq!(before, w.redactor.token("phone", "x"));
    assert_ne!(before, w.redactor.token("phone", "y"));
    assert_eq!(before, Redactor::open(w.dir.path()).unwrap().token("phone", "x"), "the key is kept on disk");

    w.redactor.rekey().unwrap();
    assert_ne!(before, w.redactor.token("phone", "x"));
}

#[test]
fn the_key_and_the_log_are_readable_only_by_their_owner() {
    use std::os::unix::fs::PermissionsExt;
    let w = World::new();
    for name in ["log.key", "PhoneLink.log"] {
        let mode = fs::metadata(w.dir.path().join(name)).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "{name}");
    }
}

fn write(store: &LogStore, byte: u8, len: u64) {
    store.make_writer().write_all(&vec![byte; usize::try_from(len).unwrap()]).unwrap();
}

fn generations(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|n| n.starts_with("PhoneLink"))
        .collect();
    names.sort();
    names
}

#[test]
fn rotation_keeps_three_generations_and_export_writes_the_oldest_first() {
    let w = World::new();
    let line = ROTATE_AT / 2 + 1;
    for &byte in b"abcd" {
        write(&w.store, byte, line);
    }

    assert_eq!(generations(w.dir.path()), ["PhoneLink.1.log", "PhoneLink.2.log", "PhoneLink.log"]);
    assert_eq!(w.store.size(), 3 * line, "the first line's generation was dropped");

    let text = w.text();
    let order: Vec<char> = text.chars().fold(Vec::new(), |mut v, c| {
        if v.last() != Some(&c) {
            v.push(c);
        }
        v
    });
    assert_eq!(order, ['b', 'c', 'd']);
}

#[test]
fn clear_empties_the_log_and_later_lines_still_land() {
    let w = World::new();
    write(&w.store, b'a', ROTATE_AT);
    write(&w.store, b'b', 10);

    w.store.clear().unwrap();
    assert_eq!(w.store.size(), 0);
    assert_eq!(generations(w.dir.path()), ["PhoneLink.log"]);

    let text = w.log(|| tracing::info!("after the clear"));
    assert!(text.ends_with(": after the clear\n"), "{text}");
}
