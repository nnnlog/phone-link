//! The `ms-phone:remoting?…` launch URI a `phoneCommand` `LaunchUri` can carry: the phone's
//! "Mirror to PC" asks the PC to open its phone screen with it.

/// The pseudo package of the phone-screen window, as the phone writes it in `packagename`.
pub const PHONE_SCREEN_PACKAGE: &str = "yourphone.phonescreen";

/// The query values of a remoting URI that the launch reads.
///
/// Every value is the query's decoded text, `None` when the query does not name it; the launch
/// decides what an empty or blank one means.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RemotingUri {
    /// `deviceid`: the phone to launch on.
    pub device_id: Option<String>,
    /// `packagename`: [`PHONE_SCREEN_PACKAGE`] or a phone app.
    pub package_name: Option<String>,
    /// `launchsource`: where the launch came from; only `Taskbar`, `StartMenu` and `HostedApp`
    /// change anything.
    pub launch_source: Option<String>,
}

impl RemotingUri {
    /// The remoting URI `uri` holds, or `None` when it is another `ms-phone` URI.
    ///
    /// `uri` must already be an absolute `ms-phone` URI. It is a remoting URI when nothing but
    /// `remoting` stands between the scheme's `:` and the query or fragment, so `ms-phone://remoting`
    /// (an authority) is not one.
    pub fn parse(uri: &str) -> Option<Self> {
        let (_, rest) = uri.split_once(':')?;
        let rest = rest.split_once('#').map_or(rest, |(before, _)| before);
        let (path, query) = rest.split_once('?').unwrap_or((rest, ""));
        if path != "remoting" {
            return None;
        }

        let values = query_values(query);
        let read = |name: &str| values.iter().find(|(n, _)| n == name).map(|(_, v)| v.clone());
        Some(Self {
            device_id: read("deviceid"),
            package_name: read("packagename"),
            launch_source: read("launchsource"),
        })
    }
}

/// The decoded `(name, value)` pairs of an HTML form query, grouped by name: a part without `=` or with an
/// empty name is skipped, and names that differ only in case are one name, spelled as it first appeared,
/// whose values are joined with `,`.
fn query_values(query: &str) -> Vec<(String, String)> {
    let mut values: Vec<(String, String)> = Vec::new();
    for part in query.split('&').filter(|p| p.contains('=')) {
        let Some((name, value)) = form_urlencoded::parse(part.as_bytes()).next() else { continue };
        if name.is_empty() {
            continue;
        }
        match values.iter_mut().find(|(n, _)| n.eq_ignore_ascii_case(&name)) {
            Some((_, joined)) => {
                joined.push(',');
                joined.push_str(&value);
            }
            None => values.push((name.into_owned(), value.into_owned())),
        }
    }
    values
}

#[cfg(test)]
mod tests;
