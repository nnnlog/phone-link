//! Unit tests of the photo file names and writes.

use super::*;

fn name(stem: &str, extension: &str) -> PhotoFileName {
    PhotoFileName { stem: stem.to_owned(), extension: extension.to_owned() }
}

#[test]
fn save_as_names_are_single_file_names() {
    assert_eq!(file_name(&name("IMG_0001", ".jpg")).as_deref(), Ok("IMG_0001.jpg"));
    assert!(file_name(&name("..", "")).is_err());
    assert!(file_name(&name("a/b", ".jpg")).is_err());
    assert!(file_name(&name("", "")).is_err());
}

#[test]
fn later_copies_are_numbered_before_the_extension() {
    assert_eq!(numbered_file_name(&name("IMG_0001", ".jpg"), 1).as_deref(), Ok("IMG_0001.jpg"));
    assert_eq!(numbered_file_name(&name("IMG_0001", ".jpg"), 2).as_deref(), Ok("IMG_0001 2.jpg"));
    assert_eq!(numbered_file_name(&name("Photo", ""), 3).as_deref(), Ok("Photo 3"));
}

#[test]
fn numbered_writes_never_overwrite() {
    let dir = std::env::temp_dir().join(format!("phonelink-numbered-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("the folder is created");
    let photo = name("IMG", ".jpg");
    let first = write_numbered(b"one", &photo, &dir).expect("the first write");
    let second = write_numbered(b"two", &photo, &dir).expect("the second write");
    assert_eq!(first.file_name().and_then(|n| n.to_str()), Some("IMG.jpg"));
    assert_eq!(second.file_name().and_then(|n| n.to_str()), Some("IMG 2.jpg"));
    assert_eq!(std::fs::read(&first).expect("the first file reads"), b"one");
    std::fs::remove_dir_all(&dir).expect("the folder is removed");
}

#[test]
fn atomic_writes_replace_the_file() {
    let dir = std::env::temp_dir().join(format!("phonelink-atomic-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("the folder is created");
    let path = dir.join("a photo.jpg");
    write_atomically(&path, b"old").expect("the first write");
    write_atomically(&path, b"new").expect("the second write");
    assert_eq!(std::fs::read(&path).expect("the file reads"), b"new");
    assert_eq!(std::fs::read_dir(&dir).expect("the folder lists").count(), 1);
    std::fs::remove_dir_all(&dir).expect("the folder is removed");
}

#[test]
fn image_bytes_name_their_format() {
    assert_eq!(image_extension(b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR"), Some("png"));
    assert_eq!(image_extension(b"\xff\xd8\xff\xe0\0\x10JFIF\0"), Some("jpg"));
    assert_eq!(image_extension(b"\0\0\0\x18ftypheic\0\0\0\0"), Some("heic"));
    assert_eq!(image_extension(b"\0\0\0\x18ftypmp42\0\0\0\0"), None);
    assert_eq!(image_extension(b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>"), None);
    assert_eq!(image_extension(b""), None);
}
