== global phone_switch: 1 migrations
-- 0
CREATE TABLE phone_switch (
        phone TEXT NOT NULL PRIMARY KEY,
        enabled INTEGER NOT NULL
    );
== global clipboard: 1 migrations
-- 0
CREATE TABLE setting (
        name TEXT NOT NULL PRIMARY KEY,
        value INTEGER NOT NULL
    );
== phone clipboard: 1 migrations
-- 0
CREATE TABLE phone_setting (
        name TEXT NOT NULL PRIMARY KEY,
        value INTEGER NOT NULL
    );
== phone audio: 1 migrations
-- 0
CREATE TABLE phone_setting (
        name TEXT NOT NULL PRIMARY KEY,
        value INTEGER NOT NULL
    );
== phone calling: 2 migrations
-- 0

CREATE TABLE call_history (
    call_id INTEGER PRIMARY KEY,
    phone_number TEXT NOT NULL,
    duration INTEGER NOT NULL,
    call_type INTEGER NOT NULL,
    start_time INTEGER NOT NULL,
    is_read INTEGER NOT NULL,
    phone_account_id TEXT NOT NULL,
    last_updated_time INTEGER NOT NULL
);
CREATE INDEX call_history_by_start ON call_history (start_time);
CREATE TABLE sequence (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    current INTEGER NOT NULL DEFAULT -1,
    expected INTEGER NOT NULL DEFAULT -1
);
INSERT INTO sequence (id) VALUES (1);
CREATE TABLE state (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    pc_consent INTEGER NOT NULL DEFAULT 0,
    auto_refresh_done INTEGER NOT NULL DEFAULT 0,
    last_updated_ms INTEGER,
    badge_mark_ms INTEGER
);
INSERT INTO state (id) VALUES (1);

-- 1

ALTER TABLE state ADD COLUMN call_control INTEGER NOT NULL DEFAULT 0;

== phone contacts: 1 migrations
-- 0

    CREATE TABLE contact (
        id INTEGER PRIMARY KEY,
        checksum INTEGER NOT NULL,
        last_updated_ms INTEGER NOT NULL,
        display_name TEXT NOT NULL,
        nickname TEXT NOT NULL,
        company TEXT NOT NULL,
        job_title TEXT NOT NULL,
        notes TEXT NOT NULL,
        name_prefix TEXT NOT NULL,
        name_suffix TEXT NOT NULL,
        given_name TEXT NOT NULL,
        middle_name TEXT NOT NULL,
        family_name TEXT NOT NULL,
        thumbnail BLOB
    );
    CREATE TABLE phone_number (
        id INTEGER PRIMARY KEY,
        contact_id INTEGER NOT NULL,
        checksum INTEGER NOT NULL,
        number TEXT NOT NULL,
        display_number TEXT,
        type INTEGER NOT NULL,
        label TEXT
    );
    CREATE INDEX phone_number_contact ON phone_number (contact_id);
    CREATE TABLE email_address (
        id INTEGER PRIMARY KEY,
        contact_id INTEGER NOT NULL,
        checksum INTEGER NOT NULL,
        address TEXT NOT NULL,
        type INTEGER NOT NULL,
        label TEXT NOT NULL
    );
    CREATE INDEX email_address_contact ON email_address (contact_id);
    CREATE TABLE postal_address (
        id INTEGER PRIMARY KEY,
        contact_id INTEGER NOT NULL,
        checksum INTEGER NOT NULL,
        type INTEGER NOT NULL,
        label TEXT NOT NULL,
        display_address TEXT NOT NULL,
        street TEXT NOT NULL,
        city TEXT NOT NULL,
        region TEXT NOT NULL,
        postal_code TEXT NOT NULL,
        country_code TEXT NOT NULL,
        sub_administrative_area TEXT NOT NULL,
        sub_locality TEXT NOT NULL
    );
    CREATE INDEX postal_address_contact ON postal_address (contact_id);
    CREATE TABLE contact_date (
        id INTEGER PRIMARY KEY,
        contact_id INTEGER NOT NULL,
        checksum INTEGER NOT NULL,
        type INTEGER NOT NULL,
        label TEXT NOT NULL,
        display_date TEXT NOT NULL
    );
    CREATE INDEX contact_date_contact ON contact_date (contact_id);
    CREATE TABLE contact_url (
        id INTEGER PRIMARY KEY,
        contact_id INTEGER NOT NULL,
        checksum INTEGER NOT NULL,
        type INTEGER NOT NULL,
        label TEXT NOT NULL,
        address TEXT NOT NULL
    );
    CREATE INDEX contact_url_contact ON contact_url (contact_id);
    CREATE TABLE sequence_number (
        record_type TEXT NOT NULL PRIMARY KEY,
        current INTEGER NOT NULL DEFAULT -1,
        expected INTEGER NOT NULL DEFAULT -1
    );
    CREATE TABLE state (
        id INTEGER NOT NULL PRIMARY KEY CHECK (id = 0),
        pc_consent INTEGER NOT NULL DEFAULT 1,
        setup_completed_once INTEGER NOT NULL DEFAULT 0,
        last_updated_ms INTEGER
    );
    INSERT INTO state (id) VALUES (0);

== phone connected_camera: 1 migrations
-- 0
CREATE TABLE phone_state (
        id INTEGER PRIMARY KEY CHECK (id = 0),
        capable INTEGER NOT NULL,
        camera_permission INTEGER NOT NULL,
        notification_permission INTEGER NOT NULL,
        camera_enabled INTEGER NOT NULL,
        initial_refresh_pending INTEGER NOT NULL,
        battery_level INTEGER NOT NULL,
        is_charging INTEGER NOT NULL
    );
== phone device_status: 1 migrations
-- 0
CREATE TABLE low_battery_notification (
        id INTEGER PRIMARY KEY CHECK (id = 0),
        enabled INTEGER NOT NULL,
        last_shown_ms INTEGER
    );
== phone file_transfer: 1 migrations
-- 0
CREATE TABLE phone_state (
        id INTEGER PRIMARY KEY CHECK (id = 0),
        share_enabled INTEGER NOT NULL,
        received_files_folder TEXT,
        show_folder_setting INTEGER NOT NULL
    );
== phone messaging: 1 migrations
-- 0

CREATE TABLE phone_state (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    capabilities INTEGER,
    mms_auto_download_on_phone INTEGER NOT NULL DEFAULT 0,
    receive_mms INTEGER NOT NULL DEFAULT 1,
    auto_download_mms INTEGER NOT NULL DEFAULT 1,
    send_mms INTEGER NOT NULL DEFAULT 1,
    pc_consent INTEGER NOT NULL DEFAULT 1,
    message_notifications INTEGER NOT NULL DEFAULT 1,
    setup_completed_once INTEGER NOT NULL DEFAULT 0,
    last_update_ms INTEGER
);
CREATE TABLE sms (
    id INTEGER PRIMARY KEY,
    checksum INTEGER,
    thread_id INTEGER NOT NULL,
    timestamp_ms INTEGER NOT NULL,
    from_address TEXT NOT NULL,
    to_address TEXT NOT NULL,
    body TEXT NOT NULL,
    subject TEXT NOT NULL,
    mailbox INTEGER NOT NULL,
    phone_read INTEGER NOT NULL,
    pc_read INTEGER NOT NULL DEFAULT 0,
    subscription_id INTEGER NOT NULL,
    group_id INTEGER NOT NULL
);
CREATE INDEX sms_thread ON sms (thread_id);
CREATE TABLE mms (
    id INTEGER PRIMARY KEY,
    checksum INTEGER,
    thread_id INTEGER NOT NULL,
    timestamp_ms INTEGER NOT NULL,
    subject TEXT NOT NULL,
    subject_charset INTEGER NOT NULL,
    mailbox INTEGER NOT NULL,
    phone_read INTEGER NOT NULL,
    pc_read INTEGER NOT NULL DEFAULT 0,
    subscription_id INTEGER NOT NULL,
    from_address TEXT NOT NULL,
    to_addresses TEXT
);
CREATE INDEX mms_thread ON mms (thread_id);
CREATE TABLE mms_part (
    part_id INTEGER PRIMARY KEY,
    message_id INTEGER NOT NULL,
    sequence INTEGER NOT NULL,
    content_id TEXT NOT NULL,
    content_type TEXT NOT NULL,
    name TEXT NOT NULL,
    text TEXT NOT NULL,
    charset INTEGER NOT NULL,
    size INTEGER NOT NULL,
    bytes BLOB
);
CREATE INDEX mms_part_message ON mms_part (message_id);
CREATE TABLE rcs_chat (
    id INTEGER PRIMARY KEY,
    checksum INTEGER,
    thread_id INTEGER NOT NULL,
    timestamp_ms INTEGER NOT NULL,
    from_address TEXT NOT NULL,
    body TEXT NOT NULL,
    content_type TEXT NOT NULL,
    session_id TEXT NOT NULL,
    mailbox INTEGER NOT NULL,
    phone_read INTEGER NOT NULL,
    pc_read INTEGER NOT NULL DEFAULT 0,
    rcs_message_type INTEGER NOT NULL,
    subscription_id INTEGER NOT NULL,
    recipients TEXT,
    user_action_required INTEGER NOT NULL
);
CREATE INDEX rcs_chat_thread ON rcs_chat (thread_id);
CREATE TABLE rcs_file_transfer (
    id INTEGER PRIMARY KEY,
    checksum INTEGER,
    thread_id INTEGER NOT NULL,
    timestamp_ms INTEGER NOT NULL,
    from_address TEXT NOT NULL,
    content_type TEXT NOT NULL,
    session_id TEXT NOT NULL,
    mailbox INTEGER NOT NULL,
    phone_read INTEGER NOT NULL,
    pc_read INTEGER NOT NULL DEFAULT 0,
    rcs_message_type INTEGER NOT NULL,
    subscription_id INTEGER NOT NULL,
    recipients TEXT,
    user_action_required INTEGER NOT NULL,
    file_name TEXT NOT NULL,
    file_size INTEGER NOT NULL,
    bytes BLOB
);
CREATE INDEX rcs_file_transfer_thread ON rcs_file_transfer (thread_id);
CREATE TABLE conversation (
    thread_id INTEGER PRIMARY KEY,
    checksum INTEGER,
    recipients TEXT NOT NULL,
    has_rcs INTEGER NOT NULL
);
CREATE TABLE rcs_conversation (
    id INTEGER PRIMARY KEY,
    checksum INTEGER,
    fallback_id INTEGER NOT NULL,
    conversation_type INTEGER NOT NULL,
    recipients TEXT NOT NULL
);
CREATE INDEX rcs_conversation_fallback ON rcs_conversation (fallback_id);
CREATE TABLE subscription (
    id INTEGER PRIMARY KEY,
    checksum INTEGER,
    row TEXT NOT NULL
);
CREATE TABLE participant (
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    thread_id INTEGER NOT NULL,
    address TEXT NOT NULL,
    normalized TEXT NOT NULL,
    UNIQUE (thread_id, normalized)
);
CREATE TABLE anchor (
    content_type TEXT PRIMARY KEY,
    current INTEGER NOT NULL DEFAULT -1,
    expected INTEGER NOT NULL DEFAULT -1
);
CREATE TABLE pending_send (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    temp_id TEXT NOT NULL UNIQUE,
    thread_id INTEGER NOT NULL,
    created_ms INTEGER NOT NULL,
    origin INTEGER NOT NULL,
    status INTEGER NOT NULL,
    failure_reason INTEGER,
    body TEXT NOT NULL,
    recipients TEXT NOT NULL,
    subscription_id INTEGER NOT NULL
);
CREATE TABLE pending_send_part (
    send_id INTEGER NOT NULL REFERENCES pending_send (id) ON DELETE CASCADE,
    idx INTEGER NOT NULL,
    name TEXT NOT NULL,
    content_type TEXT NOT NULL,
    bytes BLOB NOT NULL,
    PRIMARY KEY (send_id, idx)
);
CREATE TABLE conversation_attr (
    thread_id INTEGER PRIMARY KEY,
    muted INTEGER NOT NULL DEFAULT 0,
    hidden INTEGER NOT NULL DEFAULT 0,
    selected_subscription INTEGER
);

== phone notifications: 1 migrations
-- 0

    CREATE TABLE notifications (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        notification_id TEXT NOT NULL UNIQUE,
        package_name TEXT NOT NULL,
        json TEXT NOT NULL,
        post_time INTEGER NOT NULL,
        state INTEGER NOT NULL DEFAULT 0,
        anonymous_id TEXT NOT NULL
    );
    CREATE INDEX notifications_post_time ON notifications (post_time);
    CREATE TABLE phone_requests (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        request_id TEXT NOT NULL,
        external_id1 TEXT NOT NULL,
        external_id2 TEXT NOT NULL,
        request TEXT NOT NULL,
        time_added_ms INTEGER NOT NULL,
        finished INTEGER NOT NULL DEFAULT 0,
        time_finished_ms INTEGER,
        status TEXT,
        expired INTEGER NOT NULL DEFAULT 0
    );
    CREATE INDEX phone_requests_external_id1 ON phone_requests (external_id1);
    CREATE TABLE settings (
        setting_group_id TEXT NOT NULL,
        setting_key TEXT NOT NULL,
        setting_type INTEGER NOT NULL,
        setting_value TEXT NOT NULL,
        setting_mute_expiration_timestamp INTEGER NOT NULL DEFAULT 0,
        PRIMARY KEY (setting_group_id, setting_key)
    );
    CREATE TABLE device_data (
        name TEXT NOT NULL PRIMARY KEY,
        value INTEGER NOT NULL
    );

== phone phone_apps: 1 migrations
-- 0

CREATE TABLE apps (
    app_id INTEGER PRIMARY KEY,
    package_name TEXT NOT NULL UNIQUE,
    name TEXT NOT NULL,
    version TEXT NOT NULL,
    last_updated_time INTEGER NOT NULL,
    checksum INTEGER,
    icon BLOB,
    favorite_rank INTEGER NOT NULL DEFAULT -1
);
CREATE TABLE recent (
    recent_id INTEGER PRIMARY KEY,
    package_name TEXT NOT NULL,
    name TEXT NOT NULL,
    checksum INTEGER,
    rank INTEGER NOT NULL,
    intent_action TEXT NOT NULL,
    task_id INTEGER NOT NULL,
    web_uri TEXT,
    desktop_uri TEXT
);
CREATE TABLE sequence (
    content_type TEXT PRIMARY KEY,
    current INTEGER NOT NULL DEFAULT -1,
    expected INTEGER NOT NULL DEFAULT -1
);
INSERT INTO sequence (content_type) VALUES ('phoneApps'), ('recentApps');
CREATE TABLE state (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    last_sync_time INTEGER
);
INSERT INTO state (id) VALUES (1);

== phone phone_files: 1 migrations
-- 0
CREATE TABLE phone_state (
        id INTEGER PRIMARY KEY CHECK (id = 0),
        capable INTEGER NOT NULL,
        enabled INTEGER NOT NULL,
        permission_granted INTEGER NOT NULL,
        config BLOB,
        storage_free INTEGER NOT NULL,
        storage_total INTEGER NOT NULL,
        sequence INTEGER NOT NULL,
        initial_sync_done INTEGER NOT NULL,
        last_full_refresh INTEGER,
        last_user_interaction INTEGER,
        recycle_notice_off INTEGER NOT NULL,
        generation INTEGER NOT NULL,
        base_generation INTEGER NOT NULL
    );
    CREATE TABLE items (
        id INTEGER PRIMARY KEY,
        uri TEXT NOT NULL,
        name TEXT NOT NULL,
        location TEXT NOT NULL,
        location_key TEXT NOT NULL,
        file_type INTEGER NOT NULL,
        size INTEGER NOT NULL,
        created INTEGER,
        modified INTEGER,
        checksum INTEGER NOT NULL,
        is_directory INTEGER NOT NULL,
        generation INTEGER NOT NULL
    );
    CREATE INDEX items_location ON items (location_key);
    CREATE INDEX items_generation ON items (generation);
    CREATE TABLE folders (
        location_key TEXT PRIMARY KEY,
        location TEXT NOT NULL,
        generation INTEGER NOT NULL
    );
    CREATE TABLE tombstones (
        identifier TEXT PRIMARY KEY,
        generation INTEGER NOT NULL
    );
    CREATE TABLE thumbnails (
        id INTEGER PRIMARY KEY,
        checksum INTEGER NOT NULL,
        size INTEGER NOT NULL,
        jpeg BLOB NOT NULL
    );
== phone photos: 1 migrations
-- 0

CREATE TABLE media (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    last_updated_time INTEGER NOT NULL,
    taken_time INTEGER NOT NULL,
    orientation TEXT NOT NULL,
    last_seen_time INTEGER,
    mime_type TEXT NOT NULL,
    height INTEGER NOT NULL,
    width INTEGER NOT NULL,
    size INTEGER NOT NULL,
    uri TEXT NOT NULL,
    checksum INTEGER NOT NULL DEFAULT 0,
    thumbnail BLOB,
    media BLOB
);
CREATE INDEX media_display ON media (taken_time DESC, last_updated_time DESC, id DESC);
CREATE TABLE photos_state (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    current_sequence INTEGER NOT NULL DEFAULT -1,
    expected_sequence INTEGER NOT NULL DEFAULT -1,
    last_sync_time INTEGER,
    photos_capabilities INTEGER NOT NULL DEFAULT 0,
    consent INTEGER NOT NULL DEFAULT 1,
    deletion_allowed INTEGER NOT NULL DEFAULT 1,
    new_photo_notifications INTEGER NOT NULL DEFAULT 1,
    full_access_reminder_dismissed INTEGER NOT NULL DEFAULT 0
);
INSERT INTO photos_state (id) VALUES (1);

== phone remote_control: 1 migrations
-- 0
CREATE TABLE phone_setting (
        name TEXT NOT NULL PRIMARY KEY,
        value INTEGER NOT NULL
    );
== phone remote_snipping: 1 migrations
-- 0
CREATE TABLE phone_state (
        id INTEGER PRIMARY KEY CHECK (id = 0),
        capable INTEGER NOT NULL,
        enabled INTEGER NOT NULL,
        has_permissions INTEGER NOT NULL,
        last_popup_ms INTEGER,
        capability_answer INTEGER
    );
== phone screen_mirroring: 2 migrations
-- 0
CREATE TABLE settings (
        id INTEGER PRIMARY KEY CHECK (id = 0),
        allow_apps INTEGER NOT NULL,
        black_screen INTEGER NOT NULL,
        audio_streaming INTEGER NOT NULL,
        always_expanded INTEGER NOT NULL,
        do_not_ask_restart INTEGER NOT NULL,
        do_not_ask_task_transfer INTEGER NOT NULL,
        do_not_show_true_touch INTEGER NOT NULL,
        do_not_ask_skip_unlock INTEGER NOT NULL,
        do_not_ask_skip_verification INTEGER NOT NULL,
        do_not_ask_switch_layout INTEGER NOT NULL
    );
-- 1
CREATE TABLE packages (
        package_name TEXT PRIMARY KEY NOT NULL,
        screen_layout INTEGER NOT NULL,
        volume INTEGER NOT NULL,
        muted INTEGER NOT NULL
    );
== phone wallpaper: 1 migrations
-- 0

    CREATE TABLE phone_link_wallpaper (
        wallpaper_type INTEGER PRIMARY KEY NOT NULL,
        wallpaper_id INTEGER NOT NULL,
        blob BLOB NOT NULL);
    CREATE TABLE cdeh_wallpaper (
        wallpaper_type INTEGER PRIMARY KEY NOT NULL,
        wallpaper_id INTEGER NOT NULL,
        blob BLOB NOT NULL);
    CREATE TABLE state (
        id INTEGER PRIMARY KEY CHECK (id = 0),
        current_sequence INTEGER NOT NULL DEFAULT -1,
        expected_sequence INTEGER NOT NULL DEFAULT -1,
        device_permission INTEGER NOT NULL DEFAULT 0);
    INSERT INTO state (id) VALUES (0);

