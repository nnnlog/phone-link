use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use super::*;

#[test]
fn keyboard_layouts_are_told_from_input_methods() {
    assert_eq!(kind(Framework::IBus, "xkb:us::eng"), MirrorInputSource::Layout);
    assert_eq!(kind(Framework::IBus, "xkb:kr:kr104:kor"), MirrorInputSource::Layout);
    assert_eq!(kind(Framework::IBus, "hangul"), MirrorInputSource::InputMethod);
    assert_eq!(kind(Framework::IBus, "anthy"), MirrorInputSource::InputMethod);
    assert_eq!(kind(Framework::Fcitx5, "keyboard-us"), MirrorInputSource::Layout);
    assert_eq!(kind(Framework::Fcitx5, "keyboard-kr-kr104"), MirrorInputSource::Layout);
    assert_eq!(kind(Framework::Fcitx5, "hangul"), MirrorInputSource::InputMethod);
    assert_eq!(kind(Framework::Fcitx5, "pinyin"), MirrorInputSource::InputMethod);
}

/// A temporary directory laid out like `/proc`, removed when the test ends.
struct Proc(PathBuf);

impl Proc {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "phonelink-proc-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        Self(root)
    }

    fn process(&self, name: &str, comm: &str) -> &Self {
        let dir = self.0.join(name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("comm"), format!("{comm}\n")).unwrap();
        self
    }
}

impl Drop for Proc {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn a_framework_runs_when_a_process_of_the_user_runs_it() {
    let proc = Proc::new();
    proc.process("1", "systemd").process("self", IBUS_DAEMON).process("812", "ibus-daemon-x");
    let own = fs::metadata(&proc.0).unwrap().uid();
    assert!(!runs(&proc.0, IBUS_DAEMON, own), "only processes, by their whole name");

    proc.process("2041", IBUS_DAEMON);
    assert!(runs(&proc.0, IBUS_DAEMON, own));
    assert!(!runs(&proc.0, IBUS_DAEMON, own + 1), "another user's");
    assert!(!runs(&proc.0.join("missing"), IBUS_DAEMON, own));
}
