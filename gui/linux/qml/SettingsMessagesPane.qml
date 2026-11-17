import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import PhoneLink

// Messages: whether the phone's messages show on this computer, and the phone's contacts that name
// them (call history is in the Calls pane).
SettingsPane {
    id: pane

    readonly property var settings: {
        EngineBridge.settingsRevision
        EngineBridge.messagingRevision
        EngineBridge.contactsRevision
        return JSON.parse(EngineBridge.query("messages.settings", "")) || {}
    }
    readonly property bool connected: settings.available === true

    SettingsNote {
        visible: !pane.connected
        text: L10n.t("Settings are available once Phone Link is connected.")
    }

    SettingsSection {
        title: L10n.t("Messages")

        MacSwitch {
            Layout.fillWidth: true
            enabled: pane.connected
            text: L10n.t("Show messages on this computer")
            checked: pane.settings.showMessages === true
            onToggled: EngineBridge.perform("messages.setting", JSON.stringify({ name: "showMessages", on: checked }))
        }

        MacSwitch {
            Layout.fillWidth: true
            enabled: pane.connected && pane.settings.showMessages === true
            text: L10n.t("Notify me about new messages")
            checked: pane.settings.notify === true
            onToggled: EngineBridge.perform("messages.setting", JSON.stringify({ name: "notify", on: checked }))
        }

        MacSwitch {
            Layout.fillWidth: true
            enabled: pane.connected
            text: L10n.t("Download pictures automatically")
            checked: pane.settings.autoDownload === true
            onToggled: EngineBridge.perform("messages.setting", JSON.stringify({ name: "autoDownload", on: checked }))
        }
    }

    SettingsSection {
        title: L10n.t("Contacts")
        visible: pane.settings.contactsCapable !== false

        MacSwitch {
            Layout.fillWidth: true
            enabled: pane.connected
            text: L10n.t("Use my phone's contacts")
            checked: pane.settings.useContacts === true
            onToggled: EngineBridge.perform("messages.useContacts", String(checked))
        }

        // While this computer does not use them the phone's answer is not known.
        RowLayout {
            Layout.fillWidth: true
            visible: pane.settings.contactsAccess !== null && pane.settings.contactsAccess !== undefined
                     && pane.settings.contactsAccess !== "pcConsentRequired"
            spacing: 8

            // Lined up with the switches' labels.
            Text {
                Layout.fillWidth: true
                leftPadding: 6
                text: L10n.t("Contacts access")
                color: Theme.text
                font.pixelSize: Theme.bodySize
                opacity: pane.connected ? 1 : 0.4
                wrapMode: Text.Wrap
            }

            Text {
                visible: pane.settings.contactsAccess === "granted"
                text: L10n.t("Allowed")
                color: Theme.secondaryText
                font.pixelSize: Theme.bodySize
            }

            BusyIndicator {
                Layout.preferredWidth: 20
                Layout.preferredHeight: 20
                visible: pane.settings.contactsAccess !== "granted" && pane.settings.requestingContacts === true
                running: visible
                Accessible.name: L10n.t("Waiting for your phone")
            }

            PillButton {
                visible: pane.settings.contactsAccess !== "granted" && pane.settings.requestingContacts !== true
                enabled: pane.connected
                text: L10n.t("Send Request to Phone")
                onClicked: EngineBridge.perform("messages.requestContactsAccess", "")
            }
        }
    }
}
