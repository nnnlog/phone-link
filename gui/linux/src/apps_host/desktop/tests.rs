use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use super::exec::{ExecError, command_line};
use super::key_file::locale_keys;
use super::*;

/// A scratch tree of XDG directories, removed when dropped.
struct Tree {
    root: PathBuf,
    xdg: Xdg,
}

impl Tree {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("phonelink-desktop-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let xdg = Xdg {
            config: vec![root.join("config-home"), root.join("config-dir")],
            data: vec![root.join("data-home"), root.join("data-dir")],
            desktops: vec!["kde".to_owned()],
            path: vec![root.join("bin")],
        };
        let tree = Self { root, xdg };
        tree.program("viewer");
        tree.program("browser");
        tree
    }

    fn write(&self, relative: &str, text: &str) {
        let path = self.root.join(relative);
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("the directory is created");
        std::fs::write(path, text).expect("the file is written");
    }

    fn program(&self, name: &str) {
        self.write(&format!("bin/{name}"), "#!/bin/sh\n");
        let path = self.root.join("bin").join(name);
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("the mode is set");
    }

    fn app(&self, dir: &str, file: &str, name: &str, exec: &str, mime: &str) {
        self.write(
            &format!("{dir}/applications/{file}"),
            &format!(
                "[Desktop Entry]\nType=Application\nName={name}\nName[ko]={name} (ko)\nExec={exec}\nMimeType={mime}\n"
            ),
        );
    }

    fn ids(&self, mime: &str) -> Vec<String> {
        Applications::scan(&self.xdg, "").associated(mime).into_iter().map(|entry| entry.id).collect()
    }

    fn default_id(&self, mime: &str) -> Option<String> {
        Applications::scan(&self.xdg, "").default_for(mime).map(|entry| entry.id)
    }
}

impl Drop for Tree {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn associations_follow_the_directories_precedence() {
    let tree = Tree::new("associations");
    tree.app("data-dir", "b-viewer.desktop", "B", "viewer %f", "image/jpeg;image/png;");
    tree.app("data-dir", "a-viewer.desktop", "A", "viewer %U", "image/jpeg;");
    tree.app("data-dir", "missing.desktop", "Missing", "not-installed %f", "image/jpeg;");
    tree.app("data-home", "local.desktop", "Local", "viewer %f", "image/png;");
    tree.write("config-home/mimeapps.list", "[Added Associations]\nimage/jpeg=local.desktop;\n");
    tree.write("config-dir/kde-mimeapps.list", "[Removed Associations]\nimage/jpeg=b-viewer.desktop;\n");

    assert_eq!(tree.ids("image/jpeg"), ["local.desktop", "a-viewer.desktop"]);
    assert_eq!(tree.ids("IMAGE/PNG"), ["local.desktop", "b-viewer.desktop"]);
    assert!(tree.ids("video/mp4").is_empty());
}

#[test]
fn a_data_home_entry_shadows_the_system_one() {
    let tree = Tree::new("shadow");
    tree.app("data-dir", "viewer.desktop", "System", "viewer %f", "image/jpeg;");
    tree.write("data-home/applications/viewer.desktop", "[Desktop Entry]\nType=Application\nName=Mine\nHidden=true\n");
    assert!(tree.ids("image/jpeg").is_empty());
}

#[test]
fn the_default_is_the_first_installed_one_listed() {
    let tree = Tree::new("default");
    tree.app("data-dir", "a.desktop", "A", "viewer %f", "image/jpeg;");
    tree.app("data-dir", "b.desktop", "B", "viewer %f", "image/jpeg;");
    assert_eq!(tree.default_id("image/jpeg").as_deref(), Some("a.desktop"));

    tree.write("config-home/mimeapps.list", "[Default Applications]\nimage/jpeg=gone.desktop;b.desktop;\n");
    assert_eq!(tree.default_id("image/jpeg").as_deref(), Some("b.desktop"));
    assert_eq!(tree.default_id("image/gif"), None);
}

#[test]
fn a_scheme_handler_opens_its_uris() {
    let tree = Tree::new("schemes");
    tree.app("data-dir", "web.desktop", "Web", "browser %u", "x-scheme-handler/https;x-scheme-handler/ms-word;");
    let apps = Applications::scan(&tree.xdg, "");
    let opens = |uri: &str| uri_mime_type(uri).is_some_and(|mime| apps.default_for(&mime).is_some());
    assert!(opens("ms-word:ofv|u|https://example.com/a.docx"));
    assert!(opens("HTTPS://example.com"));
    assert!(!opens("ms-excel:ofv|u|https://example.com/a.xlsx"));
    assert!(!opens("file:///storage/emulated/0/a.docx"));
    assert!(!opens("no scheme"));
}

#[test]
fn names_follow_the_session_language() {
    let tree = Tree::new("names");
    tree.app("data-dir", "viewer.desktop", "Viewer", "viewer %f", "image/jpeg;");
    let korean = Applications::scan(&tree.xdg, "ko_KR.UTF-8");
    assert_eq!(korean.entry("viewer.desktop").map(|e| e.name).as_deref(), Some("Viewer (ko)"));
    let english = Applications::scan(&tree.xdg, "en_US.UTF-8");
    assert_eq!(english.entry("viewer.desktop").map(|e| e.name).as_deref(), Some("Viewer"));
}

#[test]
fn locale_keys_go_from_specific_to_general() {
    assert_eq!(locale_keys("sr_RS.UTF-8@latin"), ["sr_RS@latin", "sr_RS", "sr@latin", "sr"]);
    assert_eq!(locale_keys("ko_KR.UTF-8"), ["ko_KR", "ko"]);
    assert!(locale_keys("C.UTF-8").is_empty());
}

#[test]
fn file_names_get_their_globs_type() {
    let tree = Tree::new("globs");
    tree.write(
        "data-home/mime/globs2",
        "# comment\n50:image/jpeg:*.jpg\n50:image/x-photo-raw:*.raw.jpg\n50:text/x-readme:README\n",
    );
    tree.write("data-dir/mime/globs2", "50:image/png:*.png\n50:text/x-makefile:*.MK:cs\n10:image/heic:*.heic\n");
    let xdg = &tree.xdg;
    assert_eq!(mime_type_of_name(xdg, "IMG_0001.JPG").as_deref(), Some("image/jpeg"));
    assert_eq!(mime_type_of_name(xdg, "a.raw.jpg").as_deref(), Some("image/x-photo-raw"));
    assert_eq!(mime_type_of_name(xdg, "readme").as_deref(), Some("text/x-readme"));
    assert_eq!(mime_type_of_name(xdg, "build.mk"), None);
    assert_eq!(mime_type_of_name(xdg, "build.MK").as_deref(), Some("text/x-makefile"));
    assert_eq!(mime_type_of_name(xdg, "photo.heic").as_deref(), Some("image/heic"));
    assert_eq!(mime_type_of_name(xdg, "notes.txt"), None);
}

#[test]
fn noglobs_drops_lower_directories_patterns() {
    let tree = Tree::new("noglobs");
    tree.write("data-home/mime/globs2", "50:image/png:__NOGLOBS__\n");
    tree.write("data-dir/mime/globs2", "50:image/png:*.png\n");
    assert_eq!(mime_type_of_name(&tree.xdg, "a.png"), None);
}

#[test]
fn photos_videos_and_audio_are_handed_on() {
    assert!(is_media("image/jpeg"));
    assert!(is_media("video/mp4"));
    assert!(is_media("audio/ogg"));
    assert!(!is_media("image/svg+xml"));
    assert!(!is_media("application/x-desktop"));
}

fn entry(exec: &str) -> DesktopEntry {
    let text = format!("[Desktop Entry]\nType=Application\nName=Viewer\nIcon=viewer\nExec={exec}\n");
    DesktopEntry::parse("viewer.desktop", Path::new("/apps/viewer.desktop"), &text, &[]).expect("an entry")
}

fn line(exec: &str) -> Result<Vec<String>, ExecError> {
    command_line(&entry(exec), Path::new("/tmp/a b.jpg"), "file:///tmp/a%20b.jpg")
}

#[test]
fn exec_field_codes_are_filled_in() {
    assert_eq!(line("viewer %f").unwrap(), ["viewer", "/tmp/a b.jpg"]);
    assert_eq!(line("viewer --new %U").unwrap(), ["viewer", "--new", "file:///tmp/a%20b.jpg"]);
    assert_eq!(
        line("viewer %i --name=%c %k %%").unwrap(),
        ["viewer", "--icon", "viewer", "--name=Viewer", "/apps/viewer.desktop", "%"]
    );
    assert_eq!(line("viewer %d %F").unwrap(), ["viewer", "/tmp/a b.jpg"]);
}

#[test]
fn exec_quoting_keeps_arguments_whole() {
    assert_eq!(
        line(r#""/opt/My App/viewer" --title "a \"b\" %f" %f"#).unwrap(),
        ["/opt/My App/viewer", "--title", "a \"b\" %f", "/tmp/a b.jpg"]
    );
    // The string escape `\\` comes first, so a quoted `\\\\` is one backslash.
    assert_eq!(line(r#"viewer "a\\\\b" %f"#).unwrap(), ["viewer", "a\\b", "/tmp/a b.jpg"]);
    assert!(line("viewer \"open").is_err());
    assert!(line("viewer %x").is_err());
}

#[test]
fn an_entry_needs_its_program() {
    let tree = Tree::new("installed");
    tree.app("data-dir", "a.desktop", "A", "viewer %f", "image/jpeg;");
    tree.write(
        "data-dir/applications/b.desktop",
        "[Desktop Entry]\nType=Application\nName=B\nTryExec=missing\nExec=viewer %f\nMimeType=image/jpeg;\n",
    );
    tree.write(
        "data-dir/applications/c.desktop",
        "[Desktop Entry]\nType=Link\nName=C\nURL=https://example.com\nMimeType=image/jpeg;\n",
    );
    tree.write(
        "data-dir/applications/d.desktop",
        "[Desktop Entry]\nType=Application\nName=D\nDBusActivatable=true\nMimeType=image/jpeg;\n",
    );
    assert_eq!(tree.ids("image/jpeg"), ["a.desktop", "d.desktop"]);
}

#[test]
fn subdirectories_name_their_ids() {
    let tree = Tree::new("subdirectories");
    tree.app("data-dir", "kde/viewer.desktop", "Viewer", "viewer %f", "image/jpeg;");
    assert_eq!(tree.ids("image/jpeg"), ["kde-viewer.desktop"]);
}
