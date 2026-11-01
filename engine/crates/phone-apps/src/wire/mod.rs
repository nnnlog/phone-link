//! The `phoneApps` messages (phone → PC on `/legacy/phonecontent`): their keys, parsing with the
//! malformed-content rules, the PC's replies, and the rule that keeps or drops a recent entry's
//! desktop URI.
//!
//! A parse validates the whole message before anything is applied; a message it refuses is answered
//! `{ result: Int32 1, message: String "<reason> 80004005" }` and changes nothing.

use phonelink_app::Malformed;
use phonelink_app::consts::{key as app_key, result_code};
use phonelink_app::rejection_message;
use phonelink_core::codec::valueset::{Value, ValueSet};

use phonelink_features::shared::launch_gates::recent_apps_supported_by;

/// The content type of every phone-apps message.
pub const CONTENT_TYPE: &str = "phoneApps";

/// A `SyncAction` of `actions` / `recentAppsActions`: the item is deleted.
pub const ACTION_DELETE: i32 = 2;

/// The intent action of a plain app launch.
pub const MAIN_ACTION: &str = "android.intent.action.MAIN";

/// The Office app whose `http(s)` document URIs can become Office desktop URIs.
pub const OFFICE_PACKAGE: &str = "com.microsoft.office.officehubrow";

/// Packages never listed: the Link to Windows apps themselves.
pub const EXCLUDED_PACKAGES: [&str; 3] =
    ["com.microsoft.appmanager", "com.microsoft.appmanager.dev", "com.samsung.android.mdx"];

pub mod key {
    pub const MESSAGE: &str = "message";
    pub const PHONE_APPS_SETTINGS: &str = "phoneAppsSettings";
    pub const PHONE_APPS_CAPABILITIES: &str = "phoneAppsCapabilities";
    pub const PHONE_APPS_IDS: &str = "phoneAppsIds";
    pub const CHECKSUMS: &str = "checksums";
    pub const SEQUENCE_NUMBER: &str = "sequenceNumber";
    pub const RECENT_APPS_IDS: &str = "recentAppsIds";
    pub const RECENT_APPS_CHECKSUMS: &str = "recentAppsChecksums";
    pub const RECENT_APPS_SEQUENCE_NUMBER: &str = "recentAppsSequenceNumber";
    pub const APP_PACKAGE_NAMES: &str = "appPackageNames";
    pub const APP_NAMES: &str = "appNames";
    pub const APP_VERSIONS: &str = "appVersions";
    pub const LAST_UPDATED_TIMES: &str = "phoneAppsLastUpdatedTimes";
    pub const ICON_SIZES: &str = "iconSizes";
    pub const CONTENT: &str = "content";
    pub const ACTIONS: &str = "actions";
    pub const RECENT_ITEM_COUNT: &str = "recentAppsItemCount";
    pub const RECENT_PACKAGE_NAMES: &str = "recentAppsPackageNames";
    pub const RECENT_DESCRIPTIONS: &str = "recentAppsDescriptions";
    pub const RECENT_INTENT_ACTIONS: &str = "recentAppsIntentActions";
    pub const RECENT_TASK_IDS: &str = "recentAppsTaskIds";
    pub const RECENT_RANKS: &str = "recentAppsRanks";
    pub const RECENT_ACTIONS: &str = "recentAppsActions";
    pub const RECENT_WEB_URIS: &str = "recentAppsWebDocumentUris";
    pub const RECENT_DESKTOP_URIS: &str = "recentAppsWindowsDocumentUris";

    // The connect message.
    pub const CONFIG_RECENT_APPS: &str = "RecentApps";
    pub const CONFIG_APP_HANDOFF: &str = "AppHandoff";
    pub const CONFIG_EXP006: &str = "Exp006";
    /// `sequenceNumbers` entry and reply key of the app list.
    pub const SEQ_PHONE_APPS: &str = "phoneApps";
    /// `sequenceNumbers` entry and reply key of the recent list.
    pub const SEQ_RECENT_APPS: &str = "recentApps";
}

/// `phoneAppsSettings`: `None` without an `Object`; else the remoting capabilities it reports — its
/// `Int32` `phoneAppsCapabilities`, `0` when it has none.
pub fn settings(payload: &ValueSet) -> Option<u32> {
    payload.object(key::PHONE_APPS_SETTINGS).map(|s| s.i32(key::PHONE_APPS_CAPABILITIES).map_or(0, |v| v as u32))
}

/// The remoting capabilities in force for the rest of a message: its settings, else `before`.
fn effective_flags(payload: &ValueSet, before: u32) -> (Option<u32>, u32) {
    let s = settings(payload);
    (s, s.unwrap_or(before))
}

/// A complete id list of the phone with a checksum per id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdList {
    pub ids: Vec<i64>,
    /// `checksums[i]` belongs to `ids[i]` (longer checksum arrays are cut to the ids).
    pub checksums: Vec<i64>,
}

/// The reasons one id-list pair can be refused with.
struct IdListRules {
    ids_key: &'static str,
    checksums_key: &'static str,
    ids_mistyped: &'static str,
    checksums_short: &'static str,
}

const APP_ID_RULES: IdListRules = IdListRules {
    ids_key: key::PHONE_APPS_IDS,
    checksums_key: key::CHECKSUMS,
    ids_mistyped: "phoneAppsIds is not an Int64Array",
    checksums_short: "checksums missing or shorter than phoneAppsIds",
};

const RECENT_ID_RULES: IdListRules = IdListRules {
    ids_key: key::RECENT_APPS_IDS,
    checksums_key: key::RECENT_APPS_CHECKSUMS,
    ids_mistyped: "recentAppsIds is not an Int64Array",
    checksums_short: "recentAppsChecksums missing or shorter than recentAppsIds",
};

fn id_list(payload: &ValueSet, rules: &IdListRules) -> Result<Option<IdList>, Malformed> {
    let ids = match payload.get(rules.ids_key) {
        None => return Ok(None),
        Some(Value::Int64Array(ids)) => ids,
        Some(_) => return Err(Malformed::reason(rules.ids_mistyped)),
    };

    // A checksum array of another type counts as missing.
    let checksums = payload.i64_array(rules.checksums_key).unwrap_or(&[]);
    if checksums.len() < ids.len() {
        return Err(Malformed::reason(rules.checksums_short));
    }
    Ok(Some(IdList { ids: ids.clone(), checksums: checksums[..ids.len()].to_vec() }))
}

/// A metadata message (it has a `metadata` key).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Metadata {
    pub correlation_vector: String,
    /// The remoting capabilities its `phoneAppsSettings` reports.
    pub settings: Option<u32>,
    /// *Recent apps supported* once the settings are applied.
    pub recent_supported: bool,
    /// `phoneAppsIds` and `checksums`, when the message has `phoneAppsIds`.
    pub apps: Option<IdList>,
    /// An `Int64` `sequenceNumber` (read only with `phoneAppsIds`).
    pub apps_sequence: Option<i64>,
    /// `recentAppsIds` and `recentAppsChecksums`, read only with recent apps supported.
    pub recent: Option<IdList>,
    pub recent_sequence: Option<i64>,
}

/// Parses a metadata message; `remoting_before` is the phone's stored remoting capabilities.
pub fn parse_metadata(payload: &ValueSet, remoting_before: u32) -> Result<Metadata, Malformed> {
    let Some(cv) = payload.str(app_key::CORRELATION_VECTOR) else {
        return Err(Malformed::reason("metadata without a correlationVector"));
    };

    let (settings, flags) = effective_flags(payload, remoting_before);
    let recent_supported = recent_apps_supported_by(flags);

    let apps = id_list(payload, &APP_ID_RULES)?;
    // A sequence number of another type is not kept.
    let apps_sequence = apps.as_ref().and_then(|_| payload.i64(key::SEQUENCE_NUMBER));

    let (recent, recent_sequence) = if recent_supported {
        let recent = id_list(payload, &RECENT_ID_RULES)?;
        let sequence = recent.as_ref().and_then(|_| payload.i64(key::RECENT_APPS_SEQUENCE_NUMBER));
        (recent, sequence)
    } else {
        (None, None)
    };

    Ok(Metadata {
        correlation_vector: cv.to_string(),
        settings,
        recent_supported,
        apps,
        apps_sequence,
        recent,
        recent_sequence,
    })
}

/// One phone-app item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppItem {
    pub id: i64,
    pub package_name: String,
    pub name: String,
    pub version: String,
    pub last_updated: i64,
    pub checksum: i64,
    /// Its slice of `content`; `None` for an icon size of `0`.
    pub icon: Option<Vec<u8>>,
    /// Its action is `2` (delete).
    pub delete: bool,
}

/// The phone-app part of an item message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppBatch {
    pub items: Vec<AppItem>,
    /// The message carries a non-empty `actions`: deletes are by id, the rest upserted.
    pub with_actions: bool,
    pub sequence: Option<i64>,
}

/// One recent-apps entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecentEntry {
    pub id: i64,
    pub package_name: String,
    pub checksum: i64,
    /// The task's label (`""` → the app's name).
    pub description: String,
    pub intent_action: String,
    pub task_id: i64,
    pub rank: i32,
    pub delete: bool,
    /// Non-empty web document URI.
    pub web_uri: Option<String>,
    /// Non-empty desktop document URI as received (before [`desktop_uri_candidate`]).
    pub desktop_uri: Option<String>,
}

/// The recent part of an item message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecentBatch {
    pub entries: Vec<RecentEntry>,
    /// A non-empty `recentAppsActions`: the message replaces the recent list.
    pub with_actions: bool,
    pub sequence: Option<i64>,
}

/// An item message (no `metadata` key).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Items {
    pub settings: Option<u32>,
    pub recent_supported: bool,
    /// With `phoneAppsIds`.
    pub apps: Option<AppBatch>,
    /// With recent apps supported and `recentAppsIds`.
    pub recent: Option<RecentBatch>,
}

/// A required parallel array of exactly `n` entries.
fn parallel<'a, T>(
    payload: &'a ValueSet,
    get: impl Fn(&'a ValueSet) -> Option<&'a [T]>,
    n: usize,
    reason: &'static str,
) -> Result<&'a [T], Malformed> {
    match get(payload) {
        Some(v) if v.len() == n => Ok(v),
        _ => Err(Malformed::reason(reason)),
    }
}

/// An optional parallel array: another type or an empty array reads as absent; a non-empty one must
/// have exactly `n` entries.
fn optional_parallel<'a, T>(
    payload: &'a ValueSet,
    get: impl Fn(&'a ValueSet) -> Option<&'a [T]>,
    n: usize,
    reason: &'static str,
) -> Result<Option<&'a [T]>, Malformed> {
    match get(payload) {
        None => Ok(None),
        Some([]) => Ok(None),
        Some(v) if v.len() == n => Ok(Some(v)),
        Some(_) => Err(Malformed::reason(reason)),
    }
}

/// An optional `Int64` sequence number of an item message: another type is refused.
fn item_sequence(payload: &ValueSet, k: &str, reason: &'static str) -> Result<Option<i64>, Malformed> {
    match payload.get(k) {
        None => Ok(None),
        Some(Value::Int64(v)) => Ok(Some(*v)),
        Some(_) => Err(Malformed::reason(reason)),
    }
}

fn app_batch(payload: &ValueSet) -> Result<Option<AppBatch>, Malformed> {
    let ids = match payload.get(key::PHONE_APPS_IDS) {
        None => return Ok(None),
        Some(Value::Int64Array(ids)) => ids,
        Some(_) => return Err(Malformed::reason("phoneAppsIds is not an Int64Array")),
    };

    // The batch size is required; its value is not used.
    if payload.i32(app_key::ITEM_COUNT).is_none() {
        return Err(Malformed::reason("itemCount missing or not an Int32"));
    }

    let n = ids.len();
    let packages =
        parallel(payload, |p| p.string_array(key::APP_PACKAGE_NAMES), n, "appPackageNames missing or not parallel")?;
    let names = parallel(payload, |p| p.string_array(key::APP_NAMES), n, "appNames missing or not parallel")?;
    let versions = parallel(payload, |p| p.string_array(key::APP_VERSIONS), n, "appVersions missing or not parallel")?;
    let times = parallel(
        payload,
        |p| p.i64_array(key::LAST_UPDATED_TIMES),
        n,
        "phoneAppsLastUpdatedTimes missing or not parallel",
    )?;
    let checksums = parallel(payload, |p| p.i64_array(key::CHECKSUMS), n, "checksums missing or not parallel")?;
    let sizes = parallel(payload, |p| p.i64_array(key::ICON_SIZES), n, "iconSizes missing or not parallel")?;
    let Some(content) = payload.bytes(key::CONTENT) else {
        return Err(Malformed::reason("content missing or not a UInt8Array"));
    };
    let actions = optional_parallel(payload, |p| p.i32_array(key::ACTIONS), n, "actions not parallel")?;
    let sequence = item_sequence(payload, key::SEQUENCE_NUMBER, "sequenceNumber is not an Int64")?;

    let mut total: u64 = 0;
    for size in sizes {
        let size = u64::try_from(*size).map_err(|_| Malformed::reason("negative icon size"))?;
        total = total.checked_add(size).ok_or(Malformed::reason("icon sizes overflow"))?;
    }
    if total > content.len() as u64 {
        return Err(Malformed::reason("icon sizes exceed content"));
    }

    let mut offset = 0usize;
    let mut items = Vec::with_capacity(n);
    for i in 0..n {
        let size = sizes[i] as usize;
        let icon = (size != 0).then(|| content[offset..offset + size].to_vec());
        offset += size;
        items.push(AppItem {
            id: ids[i],
            package_name: packages[i].clone(),
            name: names[i].clone(),
            version: versions[i].clone(),
            last_updated: times[i],
            checksum: checksums[i],
            icon,
            delete: actions.is_some_and(|a| a[i] == ACTION_DELETE),
        });
    }
    Ok(Some(AppBatch { items, with_actions: actions.is_some(), sequence }))
}

fn recent_batch(payload: &ValueSet) -> Result<Option<RecentBatch>, Malformed> {
    let ids = match payload.get(key::RECENT_APPS_IDS) {
        None => return Ok(None),
        Some(Value::Int64Array(ids)) => ids,
        Some(_) => return Err(Malformed::reason("recentAppsIds is not an Int64Array")),
    };

    if payload.i32(key::RECENT_ITEM_COUNT).is_none() {
        return Err(Malformed::reason("recentAppsItemCount missing or not an Int32"));
    }

    let n = ids.len();
    let packages = parallel(
        payload,
        |p| p.string_array(key::RECENT_PACKAGE_NAMES),
        n,
        "recentAppsPackageNames missing or not parallel",
    )?;
    let checksums = parallel(
        payload,
        |p| p.i64_array(key::RECENT_APPS_CHECKSUMS),
        n,
        "recentAppsChecksums missing or not parallel",
    )?;
    let descriptions = parallel(
        payload,
        |p| p.string_array(key::RECENT_DESCRIPTIONS),
        n,
        "recentAppsDescriptions missing or not parallel",
    )?;
    let intents = parallel(
        payload,
        |p| p.string_array(key::RECENT_INTENT_ACTIONS),
        n,
        "recentAppsIntentActions missing or not parallel",
    )?;
    let tasks =
        parallel(payload, |p| p.i64_array(key::RECENT_TASK_IDS), n, "recentAppsTaskIds missing or not parallel")?;
    let ranks = parallel(payload, |p| p.i32_array(key::RECENT_RANKS), n, "recentAppsRanks missing or not parallel")?;
    let actions =
        optional_parallel(payload, |p| p.i32_array(key::RECENT_ACTIONS), n, "recentAppsActions not parallel")?;
    let web = optional_parallel(
        payload,
        |p| p.string_array(key::RECENT_WEB_URIS),
        n,
        "recentAppsWebDocumentUris not parallel",
    )?;
    let desktop = optional_parallel(
        payload,
        |p| p.string_array(key::RECENT_DESKTOP_URIS),
        n,
        "recentAppsWindowsDocumentUris not parallel",
    )?;
    let sequence =
        item_sequence(payload, key::RECENT_APPS_SEQUENCE_NUMBER, "recentAppsSequenceNumber is not an Int64")?;

    let non_empty = |a: Option<&[String]>, i: usize| a.map(|a| a[i].clone()).filter(|s| !s.is_empty());
    let entries = (0..n)
        .map(|i| RecentEntry {
            id: ids[i],
            package_name: packages[i].clone(),
            checksum: checksums[i],
            description: descriptions[i].clone(),
            intent_action: intents[i].clone(),
            task_id: tasks[i],
            rank: ranks[i],
            delete: actions.is_some_and(|a| a[i] == ACTION_DELETE),
            web_uri: non_empty(web, i),
            desktop_uri: non_empty(desktop, i),
        })
        .collect();
    Ok(Some(RecentBatch { entries, with_actions: actions.is_some(), sequence }))
}

/// Parses an item message; `remoting_before` is the phone's stored remoting capabilities.
pub fn parse_items(payload: &ValueSet, remoting_before: u32) -> Result<Items, Malformed> {
    let (settings, flags) = effective_flags(payload, remoting_before);
    let recent_supported = recent_apps_supported_by(flags);
    let apps = app_batch(payload)?;
    let recent = if recent_supported { recent_batch(payload)? } else { None };
    Ok(Items { settings, recent_supported, apps, recent })
}

fn has_prefix_ignore_case(s: &str, prefix: &str) -> bool {
    s.get(..prefix.len()).is_some_and(|p| p.eq_ignore_ascii_case(prefix))
}

fn has_suffix_ignore_case(s: &str, suffix: &str) -> bool {
    s.len().checked_sub(suffix.len()).and_then(|start| s.get(start..)).is_some_and(|p| p.eq_ignore_ascii_case(suffix))
}

/// A URI with a scheme: a letter, then letters, digits, `+`, `-` or `.`, then `:`.
pub fn is_absolute_uri(uri: &str) -> bool {
    let Some((scheme, _)) = uri.split_once(':') else { return false };
    let mut chars = scheme.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

/// The desktop URI a recent entry may keep, before the checks that it is an absolute URI (a message
/// with an entry whose candidate is not one is refused) and that an installed application opens it.
///
/// An `http://` or `https://` URI is dropped unless `office_rewrite` is on and the package is the
/// Office app, which turns a `docx` / `xlsx` / `pptx` URI into its Office scheme (other endings
/// dropped); any other URI is kept as it is.
pub fn desktop_uri_candidate(package: &str, uri: &str, office_rewrite: bool) -> Option<String> {
    if !(has_prefix_ignore_case(uri, "http://") || has_prefix_ignore_case(uri, "https://")) {
        return Some(uri.to_string());
    }

    if !(office_rewrite && package.eq_ignore_ascii_case(OFFICE_PACKAGE)) {
        return None;
    }

    let scheme = if has_suffix_ignore_case(uri, "docx") {
        "ms-word"
    } else if has_suffix_ignore_case(uri, "xlsx") {
        "ms-excel"
    } else if has_suffix_ignore_case(uri, "pptx") {
        "ms-powerpoint"
    } else {
        return None;
    };
    Some(format!("{scheme}:ofv|u|{uri}"))
}

/// The metadata reply: the requested ids; `recent` only with recent apps supported.
pub fn metadata_reply(
    correlation_vector: &str,
    contract_version: f64,
    apps: &[i64],
    recent: Option<&[i64]>,
) -> ValueSet {
    let count = apps.len() + recent.map_or(0, <[i64]>::len);
    let mut vs = ValueSet::new()
        .with(app_key::CORRELATION_VECTOR, correlation_vector)
        .with(app_key::CONTENT_TYPE, CONTENT_TYPE)
        .with(app_key::CONTRACT_VERSION, contract_version)
        .with(app_key::ITEM_COUNT, i32::try_from(count).unwrap_or(i32::MAX))
        .with(key::PHONE_APPS_IDS, apps.to_vec())
        .with(app_key::RESULT, result_code::SUCCESS);
    if let Some(recent) = recent {
        vs.insert(key::RECENT_APPS_IDS, recent.to_vec());
    }
    vs
}

/// The answer to a refused message: `{ result: 1, message: "<reason> 80004005" }`.
pub fn rejection_answer(reason: &'static str) -> ValueSet {
    ValueSet::new().with(app_key::RESULT, result_code::FAILURE).with(key::MESSAGE, rejection_message(reason))
}

#[cfg(test)]
mod tests;
