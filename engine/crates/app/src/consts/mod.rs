//! Vocabulary of the App message layer: routes, `ValueSet` keys, the `contentType` catalog, the
//! `result` codes and the flag sets the layer reads and writes.

/// `_route` values this layer owns.
pub mod route {
    /// The connect message.
    pub const CONNECT: &str = "/connect/v1";
    /// The permission prompt request.
    pub const PERMISSION: &str = "/permission";
    /// The only route inbound App requests are processed on.
    pub const PHONE_CONTENT: &str = "/legacy/phonecontent";
    /// Responses, and the rejection of a request.
    pub const RESPONSE: &str = "/internal/response";
    /// DRM requests are not `ValueSet` requests; this layer does not build them.
    pub const DEVICE_RESOURCE_MANAGER: &str = "/DeviceResourceManager";
}

/// `ValueSet` keys of the shared layer: outbound requests, responses and inbound dispatch.
pub mod key {
    // Standard keys of every request.
    pub const DISPLAY_NAME: &str = "displayName";
    pub const APP_VERSION: &str = "appVersion";
    pub const INSTALLATION_ID: &str = "installationId";
    pub const CONTRACT_VERSION: &str = "contractVersion";
    // Legacy-request keys.
    pub const IS_USER_INITIATED: &str = "isuserinitiated";
    pub const IGNORE_RESPONSE: &str = "ignoreResponse";
    pub const TRANSPORT_TYPE: &str = "transportType";
    // Feature-owned common keys.
    pub const CONTENT_TYPE: &str = "contentType";
    pub const CORRELATION_VECTOR: &str = "correlationVector";
    // Keys lifted from the payload when the envelope lacks them.
    pub const RESULT: &str = "result";
    pub const REQUEST_ID: &str = "_requestId";
    pub const ROUTE: &str = "_route";
    // Inbound dispatch.
    pub const METADATA: &str = "metadata";
    pub const SYNC_ID: &str = "syncID";
    // The connect message containers.
    pub const SEQUENCE_NUMBERS: &str = "sequenceNumbers";
    pub const CONFIGURATION: &str = "configuration";
    pub const ATTRIBUTES: &str = "attributes";
    // Shared App messages.
    pub const ITEM_COUNT: &str = "itemCount";
    pub const PERMISSION_TYPES: &str = "permissionTypes";
    pub const PERMISSION_STATES: &str = "permissionStates";
    pub const PERMISSION_TYPE: &str = "permissionType";
    pub const SILENT: &str = "silent";
    pub const PERMISSION_SESSION_ID: &str = "permissionSessionId";
    pub const PHONE_COMMAND_TYPE: &str = "phoneCommandType";
    pub const SYNC_OVER_METERED_CONNECTION: &str = "syncOverMeteredConnection";
    pub const LAUNCH_URI: &str = "launchURI";
    pub const HOTSPOT_USER_CONSENT: &str = "hotspotUserConsent";
    pub const OEM_BLUETOOTH_PERMISSION: &str = "oemBluetoothPermission";
    pub const OEM_BLUETOOTH_RFCOMM_ENABLED: &str = "oemBluetoothRfcommEnabled";
    pub const POST_NOTIFICATION_SETTING_SCENARIO: &str = "postNotificationSettingScenario";
    pub const NOTIFICATIONS_SENSITIVE_PERMISSION_SCENARIO: &str = "notificationsSensitivePermissionScenario";
}

/// The `contentType` strings the PC itself sends or answers with that are **not** in the
/// `MediaType` catalog.
pub mod literal {
    pub const CONNECT: &str = "connect";
    pub const PERMISSION: &str = "permission";
    /// The `contentType` of the `permissions` metadata answer (capital P).
    pub const PERMISSIONS_ANSWER: &str = "Permissions";
}

/// The `MediaType` catalog: index = the `MediaType` number. Lookup is exact and case-sensitive; any
/// other string — and a missing `contentType` — is classified as `test`, which has no processor.
pub mod content_type {
    pub const TEST: &str = "test";
    pub const HEARTBEAT: &str = "heartbeat";
    pub const DEVICE_QUERY: &str = "deviceQuery";
    pub const PHONE_COMMAND: &str = "phoneCommand";
    pub const PERMISSIONS: &str = "permissions";

    pub const CATALOG: [&str; 50] = [
        "test",
        "photos",
        "messages",
        "phoneApps",
        "deviceQuery",
        "screenMirror",
        "contacts",
        "messageAck",
        "messagesMms",
        "heartbeat",
        "deviceStatus",
        "messagesMmsMedia",
        "conversations",
        "bluetoothState",
        "notifications",
        "settings",
        "contactThumbs",
        "messaging",
        "subscriptions",
        "callLogs",
        "messagesRcsChat",
        "wallpaper",
        "thumbnails",
        "mediaInfo",
        "sharedContent",
        "copypaste_metadata",
        "copypaste_payload",
        "dragdrop_metadata",
        "dragdrop_payload",
        "phoneCommand",
        "messagesRcsFileTransfer",
        "conversationsRcs",
        "contactsV2",
        "dialOnPhone",
        "sharedContentV2",
        "audioCommand",
        "phoneNumbers",
        "emailAddresses",
        "postalAddresses",
        "contactDates",
        "contactUrls",
        "deviceStatusCommand",
        "recentApps",
        "permissions",
        "continuity",
        "contactsVCard",
        "callHistoryPbap",
        "continuityAppContext",
        "remoteControl",
        "fileTransfer",
    ];

    /// The catalog name an inbound `contentType` is classified as.
    pub fn classify(raw: Option<&str>) -> &'static str {
        raw.and_then(|s| CATALOG.iter().find(|c| **c == s).copied()).unwrap_or(TEST)
    }

    #[cfg(feature = "test-support")]
    /// Whether `name` is a catalog entry (exact match).
    pub fn is_catalog_name(name: &str) -> bool {
        CATALOG.contains(&name)
    }
}

/// The `result` codes.
pub mod result_code {
    pub const SUCCESS: i32 = 0;
    pub const FAILURE: i32 = 1;
    pub const UNKNOWN_TYPE: i32 = 2;
    pub const INCOMPATIBLE_VERSIONS: i32 = 3;
    pub const USER_ACCEPTED: i32 = 4;
    pub const USER_DENIED: i32 = 5;
    pub const USER_PERMANENTLY_DENIED: i32 = 6;
    pub const NEEDS_PERMISSION: i32 = 7;
    pub const METERED_CONNECTION: i32 = 8;
    pub const NEEDS_INITIAL_PERMISSION: i32 = 9;
    pub const CANCELED_DISCONNECTED: i32 = 10;
    pub const CANCELED_COALESCED: i32 = 11;
    pub const METERED_CONNECTION_SYNC_DISABLED: i32 = 12;
    pub const ALL_FEATURES_DISABLED: i32 = 13;
    pub const INVALID_REQUEST: i32 = 14;
    pub const DATABASE_OPERATION_FAILED: i32 = 15;
    pub const FEATURE_FLAG_DISABLED: i32 = 16;
    pub const DEVICE_RESOLUTION_FAILED: i32 = 17;
    pub const DEVICE_NOT_SUPPORTED: i32 = 18;
    pub const USER_SESSION_INACTIVE: i32 = 19;
    pub const APP_LAUNCHED: i32 = 20;
    pub const TWO_PHONE_MODE_SYNC_DISABLED: i32 = 21;
    pub const USER_REVOKED_TRUST: i32 = 22;
    pub const NON_DEFAULT_USER_PROFILE: i32 = 23;
    /// What a missing or non-integer `result` reads as.
    pub const MISSING: i32 = -1;
}

/// `DevicePermissions` flags, as the phone's `permissions` message reports them.
pub mod permission {
    pub const PHOTOS: u32 = 0x1;
    pub const MESSAGES: u32 = 0x2;
    pub const MIRROR: u32 = 0x4;
    pub const PHONE_APPS: u32 = 0x8;
    pub const CALLS: u32 = 0x10;
    pub const BLUETOOTH_LE_PAIR: u32 = 0x20;
    pub const NOTIFICATIONS_SYNC: u32 = 0x40;
    pub const BLUETOOTH_ADVERTISE: u32 = 0x80;
    pub const CALL_LOGS: u32 = 0x100;
    pub const SHARED_CONTENT: u32 = 0x400;
    pub const CONTACTS: u32 = 0x800;
    pub const PHOTOS_DELETE: u32 = 0x20_0000;
    pub const CAMERA: u32 = 0x40_0000;
    pub const FILES: u32 = 0x80_0000;
    pub const BLUETOOTH: u32 = 0x200_0000;
    pub const POST_NOTIFICATIONS: u32 = 0x800_0000;
    pub const AUTO_LAUNCH: u32 = 0x1000_0000;
    pub const PHOTOS_FULL: u32 = 0x2000_0000;
    pub const READ_MEDIA_IMAGES: u32 = 0x4000_0000;
    /// Sent as the `Int32` `-2147483648`.
    pub const NOTIFICATIONS_SENSITIVE: u32 = 0x8000_0000;
}

/// `DeviceCapabilities` — the phone capability bits `registeredPermissionTypes` of a connect response.
pub mod capability {
    pub const PHOTOS: u32 = 0x1;
    pub const MESSAGES: u32 = 0x2;
    pub const MIRROR: u32 = 0x4;
    pub const PHONE_APPS: u32 = 0x8;
    pub const CALLS: u32 = 0x10;
    pub const NOTIFICATIONS_LISTENER: u32 = 0x40;
    pub const CALL_LOGS: u32 = 0x100;
    pub const WALLPAPER: u32 = 0x200;
    pub const SHARED_CONTENT: u32 = 0x400;
    pub const CONTACTS: u32 = 0x800;
    pub const AUDIO_CONTROL: u32 = 0x4000;
    pub const DIAL_ON_PHONE: u32 = 0x2_0000;

    /// The capability bits of a new phone record written from the device list (Android).
    pub const NEW_RECORD: u32 = CALLS;
    /// Android phone default: Photos, Messages, Mirror, `PhoneApps`, Calls, `NotificationsListener`,
    /// `CallLogs`, Wallpaper, `SharedContent`, Contacts, `AudioControl`.
    pub const ANDROID_PHONE_DEFAULT: u32 = 0x4F5F;
    /// Android tablet default: the phone set without Messages, Calls, `CallLogs`.
    pub const ANDROID_TABLET_DEFAULT: u32 = 0x4E4D;
    /// Up to this contract version an Android phone gets Calls added.
    pub const CALLS_ADDED_UP_TO_CONTRACT: f64 = 3.93;
}

/// The flags of the connect response's `yppCapabilities`.
pub mod ypp {
    pub const CRYPTO_TRUST: u32 = 0x1;
    pub const PAIRING: u32 = 0x2;
    pub const MANUAL_CONNECT: u32 = 0x4;
    pub const SIDE_CHANNEL_OEM_SUPPORT: u32 = 0x8;
    pub const A2D_MIGRATION: u32 = 0x10;
    pub const ASYNC_TRUST: u32 = 0x20;
    pub const INTENTIONAL_DISCONNECT: u32 = 0x40;

    /// Flag names as `customData` spells them, for its name form.
    pub const NAMES: [(&str, u32); 7] = [
        ("CryptoTrust", CRYPTO_TRUST),
        ("Pairing", PAIRING),
        ("ManualConnect", MANUAL_CONNECT),
        ("SideChannelOemSupport", SIDE_CHANNEL_OEM_SUPPORT),
        ("A2DMigration", A2D_MIGRATION),
        ("AsyncTrust", ASYNC_TRUST),
        ("IntentionalDisconnect", INTENTIONAL_DISCONNECT),
    ];

    /// The YPP version an Android record holds when the device list has none.
    pub const ANDROID_DEFAULT_VERSION: i32 = 8;
}

/// `DeviceAssetCategory`, the connect response's `attributes.assetCategory`.
pub mod asset_category {
    pub const UNKNOWN: u32 = 0;
    pub const PHONE: u32 = 1;
    pub const TABLET: u32 = 2;
    pub const FOLD_VERTICAL: u32 = 3;
    pub const FOLD_HORIZONTAL: u32 = 4;

    pub const NAMES: [(&str, u32); 5] = [
        ("Unknown", UNKNOWN),
        ("Phone", PHONE),
        ("Tablet", TABLET),
        ("FoldVertical", FOLD_VERTICAL),
        ("FoldHorizontal", FOLD_HORIZONTAL),
    ];
}

/// The connect response's `attributes.platform`.
pub mod platform {
    pub const UNKNOWN: i32 = 0;
    pub const ANDROID: i32 = 1;
    pub const IOS: i32 = 2;
}

/// Device-status values of the phone record: status capabilities, and the defaults a record written
/// from the device list (Android) holds.
pub mod status {
    /// `statusCapabilities` of an Android phone whose device list has none while the stored value is 0.
    pub const ANDROID_DEFAULT_CAPABILITIES: u32 = 0x17FF;
    /// Android SDK version an Android record holds when the device list has none.
    pub const ANDROID_DEFAULT_SDK_VERSION: u32 = 24;
}

/// Message priorities.
pub mod priority {
    pub const HIGH: u32 = 0;
    pub const MEDIUM: u32 = 50;
    pub const LOW: u32 = 100;
    /// Inbound answers.
    pub const RESPONSE: u32 = MEDIUM;
}

/// The HRESULT the engine's `message` text ends with when it rejects malformed content.
pub const E_FAIL_TEXT: &str = "80004005";

#[cfg(test)]
mod tests;
