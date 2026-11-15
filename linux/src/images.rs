use std::path::PathBuf;

fn cache_dir() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME")
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
        .unwrap_or_else(std::env::temp_dir)
        .join("phonelink")
}

fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| (hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3))
}

fn file_url(path: &std::path::Path) -> String {
    format!("file://{}", path.display())
}

fn safe_name(key: &str) -> String {
    key.chars().map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' { c } else { '_' }).collect()
}

/// The cached file for `key` in `kind`, written from `load` the first time it is asked for.
pub fn cached(kind: &str, key: &str, load: impl FnOnce() -> Option<Vec<u8>>) -> Option<String> {
    let path = cache_dir().join(kind).join(safe_name(key));
    if path.exists() {
        return Some(file_url(&path));
    }
    let bytes = load()?;
    std::fs::create_dir_all(path.parent()?).ok()?;
    std::fs::write(&path, bytes).ok()?;
    Some(file_url(&path))
}

/// A file named by the content's hash, for images that change under one key.
pub fn by_content(kind: &str, bytes: &[u8]) -> Option<String> {
    cached(kind, &format!("{:016x}", fnv1a(bytes)), || Some(bytes.to_vec()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_keep_only_safe_characters() {
        assert_eq!(safe_name("com.kakao.talk"), "com.kakao.talk");
        assert_eq!(safe_name("../x y"), ".._x_y");
    }
}
