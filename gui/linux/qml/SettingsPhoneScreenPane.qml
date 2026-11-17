import QtQuick
import QtQuick.Layouts
import PhoneLink

// Phone Screen: whether the phone's screen and apps open on this computer, how the phone behaves
// meanwhile, its audio, and the questions the windows ask before acting.
SettingsPane {
    id: pane

    readonly property var status: { EngineBridge.mirroringRevision; return JSON.parse(EngineBridge.query("mirror.status", "")) }
    readonly property bool offered: status !== null && status.offered === true
    readonly property var settings: offered ? status.settings : ({})
    readonly property bool allowApps: settings.allowApps === true

    function setting(key, value) {
        EngineBridge.perform("mirror.setting", JSON.stringify({ key: key, value: value }))
    }

    SettingsSection {
        title: L10n.t("Phone Screen")
        visible: !pane.offered

        Text {
            Layout.fillWidth: true
            text: pane.status === null ? L10n.t("Settings are available once Phone Link is connected.")
                                       : L10n.t("This phone's screen can't be shown on this computer.")
            color: Theme.secondaryText
            font.pixelSize: Theme.bodySize
            wrapMode: Text.Wrap
        }
    }

    SettingsSection {
        title: L10n.t("Phone Screen")
        visible: pane.offered

        MacSwitch {
            Layout.fillWidth: true
            text: L10n.t("Open your phone's screen and apps on this computer")
            checked: pane.allowApps
            onToggled: pane.setting("allowApps", checked)
        }

        SettingsNote {
            Layout.topMargin: -8
            text: L10n.t("Turning this off closes their windows")
        }

        MacSwitch {
            Layout.fillWidth: true
            text: L10n.t("Turn your phone's screen black while it's shown here")
            enabled: pane.allowApps
            checked: pane.settings.blackScreen === true
            onToggled: pane.setting("blackScreen", checked)
        }

        MacSwitch {
            Layout.fillWidth: true
            visible: pane.status !== null && pane.status.audio === true
            text: L10n.t("Play apps' audio on this computer")
            enabled: pane.allowApps
            checked: pane.settings.audioStreaming === true
            onToggled: pane.setting("audioStreaming", checked)
        }

        MacSwitch {
            Layout.fillWidth: true
            visible: pane.status !== null && pane.status.adaptiveLayout === true
            text: L10n.t("Always open apps in the expanded layout")
            enabled: pane.allowApps
            checked: pane.settings.alwaysExpanded === true
            onToggled: pane.setting("alwaysExpanded", checked)
        }
    }

    // Each switch shows its question being asked: on while the setting's "do not ask" is off.
    SettingsSection {
        title: L10n.t("Ask Before Continuing")
        visible: pane.offered

        Repeater {
            model: [
                { key: "doNotAskRestart", text: L10n.t("When your phone moves an app to this computer"), shown: true },
                { key: "doNotAskTaskTransfer", text: L10n.t("When your phone hands off what you're doing to this computer"), shown: true },
                { key: "doNotShowTrueTouch", text: L10n.t("When an app checks that you're touching your phone"), shown: true },
                { key: "doNotAskSwitchLayout", text: L10n.t("When you change an app's layout"),
                  shown: pane.status !== null && pane.status.adaptiveLayout === true },
                { key: "doNotAskSkipUnlock", text: L10n.t("When connecting without unlocking your phone"), shown: true },
                { key: "doNotAskSkipVerification", text: L10n.t("When your phone has been verified with a code"), shown: true }
            ]

            MacSwitch {
                required property var modelData
                Layout.fillWidth: true
                visible: modelData.shown
                text: modelData.text
                enabled: pane.allowApps
                checked: pane.settings[modelData.key] !== true
                onToggled: pane.setting(modelData.key, !checked)
            }
        }
    }
}
