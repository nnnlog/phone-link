import QtQuick
import QtQuick.Layouts
import PhoneLink

// The media controls rows of the General settings, placed in one of its sections: whether what plays
// on the phone shows in the app (off while it does not apply), and whether it also shows in the
// desktop's media controls and answers the media keys.
ColumnLayout {
    id: rows

    readonly property var media: { EngineBridge.mediaRevision; return JSON.parse(EngineBridge.query("media", "")) || {} }

    Layout.fillWidth: true
    spacing: 12

    MacSwitch {
        Layout.fillWidth: true
        text: L10n.t("Show what plays on the phone")
        enabled: rows.media.settingApplies === true
        checked: rows.media.showsControls === true
        onToggled: {
            EngineBridge.perform("media.setting", String(checked))
            checked = Qt.binding(() => rows.media.showsControls === true)
        }
    }

    SettingsNote {
        text: rows.media.settingDetail || L10n.t("Control what plays on your phone from this computer.")
    }

    MacSwitch {
        Layout.fillWidth: true
        text: L10n.t("Use media keys and the desktop's media controls")
        enabled: rows.media.showsControls === true
        checked: rows.media.systemControls === true
        onToggled: {
            EngineBridge.perform("media.systemControls", String(checked))
            checked = Qt.binding(() => rows.media.systemControls === true)
        }
    }

    SettingsNote {
        text: L10n.t("While your phone has media open, the keyboard's play, pause and skip keys and the desktop's media controls control it.")
    }
}
