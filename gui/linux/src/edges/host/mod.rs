//! The computer's name and region for the engine: the host name, and the territory of the locale
//! that holds the Region setting.

use phonelink_engine::edge::HostInfo;

#[cfg(test)]
mod tests;

/// The locale variables that hold the Region setting, in order of precedence: `LC_ALL`, the region
/// categories that the desktops' Region (Formats) setting writes, then `LANG`. POSIX gives `LC_ALL`
/// precedence over a category's own variable, and that over `LANG`; the order among `LC_TELEPHONE`,
/// `LC_ADDRESS` and `LC_MONETARY` is this app's own choice. `LC_MESSAGES`, the display language, is
/// not one of them.
const REGION_VARIABLES: [&str; 5] = ["LC_ALL", "LC_TELEPHONE", "LC_ADDRESS", "LC_MONETARY", "LANG"];

/// The computer as the engine describes it to the phone and the account.
pub struct LinuxHost;

impl HostInfo for LinuxHost {
    /// The host name from `gethostname`.
    #[expect(unsafe_code, reason = "gethostname has no safe wrapper in the dependencies")]
    fn host_name(&self) -> String {
        let mut buffer = [0u8; libc::HOST_NAME_MAX as usize + 1];
        // SAFETY: the pointer and length describe `buffer`, which the call writes within.
        let result = unsafe { libc::gethostname(buffer.as_mut_ptr().cast(), buffer.len()) };
        // A buffer of HOST_NAME_MAX + 1 bytes holds every host name, so the call cannot fail.
        assert!(result == 0, "gethostname failed: {}", std::io::Error::last_os_error());
        name_in(&buffer)
    }

    fn region_code(&self) -> Option<String> {
        region_of_environment(|name| std::env::var(name).ok())
    }
}

/// The text before the first NUL of `buffer` (all of it without one), with invalid UTF-8 replaced by
/// U+FFFD.
fn name_in(buffer: &[u8]) -> String {
    let name = buffer.iter().position(|&byte| byte == 0).map_or(buffer, |end| &buffer[..end]);
    String::from_utf8_lossy(name).into_owned()
}

/// The region of the locale in the first of `REGION_VARIABLES` that `variable` gives a non-empty
/// value; `None` when that locale names no region.
fn region_of_environment(variable: impl Fn(&str) -> Option<String>) -> Option<String> {
    REGION_VARIABLES
        .into_iter()
        .filter_map(variable)
        .find(|value| !value.is_empty())
        .and_then(|locale| region_of_locale(&locale))
}

/// The territory of `locale` (`ko_KR.UTF-8` → `KR`); `None` when it names no two-letter one.
fn region_of_locale(locale: &str) -> Option<String> {
    let region = locale.split(['.', '@']).next()?.split_once('_')?.1;
    (region.len() == 2 && region.chars().all(|c| c.is_ascii_alphabetic())).then(|| region.to_ascii_uppercase())
}
