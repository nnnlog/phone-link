//! Which log fields are written as they are.
//!
//! A field is written as it is only when its name is on the public list below or has one of the
//! public shapes ([`PUBLIC_SUFFIXES`], [`PUBLIC_PREFIXES`]); a free-text field or a `raw_*` one is
//! scrubbed of identifier shapes ([`FREE_TEXT`], [`RAW_PREFIX`]); every other field is replaced by a
//! redaction token, so a field nobody classified can never put a value in the file. A name is
//! matched on its last `.`-separated part (`%ctx.phone` records the field `ctx.phone`).
//!
//! Name a field, or add a name to [`PUBLIC`], so that it is public only when every value logged
//! under it is a count, size, duration, flag, enum, protocol constant, or a string the code itself
//! chose — never an identifier, a name, a path, an address, or text that came from the phone or a
//! server. An identifier that helps to follow a flow stays off the list: its token carries a tag that
//! is equal for equal values.

/// How the log writes a field's value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FieldClass {
    /// As it is.
    Public,
    /// As it is after [`super::redact::scrub`] replaces the identifier shapes it finds.
    FreeText,
    /// As a redaction token.
    Redacted,
}

/// The class of the field `name`.
pub(crate) fn class_of(name: &str) -> FieldClass {
    let last = name.rsplit('.').next().unwrap_or(name);
    if last.starts_with(RAW_PREFIX) || FREE_TEXT.contains(&last) {
        FieldClass::FreeText
    } else if PUBLIC.contains(&last)
        || PUBLIC_SUFFIXES.iter().any(|s| last.ends_with(s))
        || PUBLIC_PREFIXES.iter().any(|p| last.starts_with(p))
    {
        FieldClass::Public
    } else {
        FieldClass::Redacted
    }
}

/// The beginning of a name for a string as it arrived from the phone or a server (`raw_route`,
/// `raw_content_type`): scrubbed like an error text whatever else the name says, because a value the
/// code did not match to one it knows is whatever the sender put there.
const RAW_PREFIX: &str = "raw_";

/// Error and explanation texts. The engine's own texts do not quote the identifiers it holds, except
/// a few that name a stored key or a store's path, which end in a GUID or sit under the home folder;
/// a library's error or a panic may quote anything. So these are scrubbed rather than trusted.
const FREE_TEXT: &[&str] =
    &["error", "reason", "failure", "detail", "why", "cause", "panic", "fault", "rejection", "what"];

/// Name endings that say the value is a count, a size, a duration, or an enum the code defines
/// (`file_count`, `frame_len`, `elapsed_ms`, `trust_type`, `drag_phase`).
const PUBLIC_SUFFIXES: &[&str] =
    &["_count", "_len", "_bytes", "_size", "_ms", "_secs", "_type", "_kind", "_state", "_phase", "_status", "_outcome"];

/// Name beginnings that say the value is a flag (`is_secure`, `has_permissions`).
const PUBLIC_PREFIXES: &[&str] = &["is_", "has_"];

const PUBLIC: &[&str] = &[
    // Outcomes, states, and enums the code defines.
    "outcome",
    "status",
    "result",
    "state",
    "kind",
    "code",
    "step",
    "phase",
    "gate",
    "action",
    "command",
    "event",
    "case",
    "format",
    "transport",
    "flow",
    "trigger",
    "skipped",
    "last_failure",
    "launch_result",
    "probe_result",
    "types",
    "contract",
    "contract_version",
    "version",
    "error_no",
    "results",
    "env",
    "category",
    // Protocol constants and names the code chose: routes, content types, workflows, OAuth scopes.
    "route",
    "wire_key",
    "bluetooth_scenario",
    "service_uuid",
    "workflow",
    "feature",
    "call",
    "scope",
    "permission",
    // Counts, sizes, positions, and durations.
    "n",
    "count",
    "len",
    "bytes",
    "size",
    "sizes",
    "largest",
    "items",
    "sends",
    "apps",
    "targets",
    "scopes",
    "candidates",
    "upserts",
    "deletes",
    "deleted",
    "removed",
    "missed",
    "misses",
    "lost",
    "attempt",
    "attempts",
    "tries",
    "try_number",
    "fragment_id",
    "index",
    "channel",
    "frame_id",
    "sequence",
    "sequence_number",
    "starting_sequence",
    "initial_send_sn",
    "offset",
    "position",
    "width",
    "height",
    "percent",
    "bps",
    "limit",
    "threshold",
    "queue",
    "priority",
    "bit",
    "delay",
    "ttl",
    "lifetime",
    "epoch",
    "expires",
    "estimated",
    "end",
    "flags",
    // Flags.
    "enabled",
    "ok",
    "failed",
    "waiting",
    "started",
    "starting",
    "completed",
    "requested",
    "changed",
    "granted",
    "grant",
    "capable",
    "available",
    "silent",
    "cached",
    "read",
    "found",
    "open",
    "opened",
    "locked",
    "secure",
    "live",
    "on_hub",
    "resume",
    "resumable",
    "restart",
    "retry",
    "reconnect",
    "allow_reconnect",
    "repeated",
    "outgoing",
    "recent",
    "modified",
    "refreshed",
    "validated",
    "sent",
    "listed",
    "missing",
    "bound",
    "flush",
    "interactive",
    "user_initiated",
    "pc_initiated",
    "force_session_start",
    "only_stun",
    "stun",
    "turn",
    "ipv6",
    "internet_available",
    "skip_unlock",
    "skip_verification",
    "inline_ux",
    "talkback",
    "negative",
    "layout",
    "home",
    "lock",
    "low",
    "lower",
    "expected",
    "want",
    "failed_notice",
];
