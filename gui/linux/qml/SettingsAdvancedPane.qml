import QtQuick
import QtQuick.Layouts
import PhoneLink

// Advanced: this computer's registration with the Microsoft account, a reset that registers it again
// at the next sign-in, and the app's log.
SettingsPane {
    id: pane

    signal signedOut()

    readonly property var registration: {
        EngineBridge.settingsRevision
        return JSON.parse(EngineBridge.query("account.registration", "")) || {}
    }
    readonly property var account: {
        EngineBridge.settingsRevision
        EngineBridge.phase
        return JSON.parse(EngineBridge.query("shell.account", "")) || {}
    }

    // On screen: the current tab of an open Settings window.
    readonly property bool shown: visible && Window.window !== null && Window.window.visible

    // The registration is read off the GUI thread each time the pane shows.
    onShownChanged: {
        if (shown)
            EngineBridge.perform("account.loadRegistration", "")
    }

    SettingsSection {
        title: L10n.t("Device Registration")

        Text {
            Layout.fillWidth: true
            visible: pane.registration.loaded === true && pane.registration.registered !== true
            text: L10n.t("This computer is not registered yet.")
            color: Theme.secondaryText
            font.pixelSize: Theme.bodySize
        }

        GridLayout {
            Layout.fillWidth: true
            visible: pane.registration.registered === true
            columns: 2
            columnSpacing: 16
            rowSpacing: 8

            Text {
                text: L10n.t("Device ID")
                color: Theme.text
                font.pixelSize: Theme.bodySize
            }

            TextEdit {
                Layout.fillWidth: true
                text: pane.registration.deviceId || ""
                color: Theme.secondaryText
                font.family: "monospace"
                font.pixelSize: Theme.subheadSize
                horizontalAlignment: TextEdit.AlignRight
                wrapMode: TextEdit.WrapAnywhere
                readOnly: true
                selectByMouse: true
                selectionColor: Theme.accent
                selectedTextColor: "white"
            }

            Repeater {
                model: (pane.registration.linkedIds || []).length > 0 ? pane.registration.linkedIds : [""]

                delegate: RowLayout {
                    required property string modelData

                    Layout.columnSpan: 2
                    Layout.fillWidth: true
                    spacing: 16

                    Text {
                        text: L10n.t("Linked ID")
                        color: Theme.text
                        font.pixelSize: Theme.bodySize
                    }

                    TextEdit {
                        Layout.fillWidth: true
                        text: parent.modelData.length > 0 ? parent.modelData : L10n.t("Not yet listed")
                        color: Theme.secondaryText
                        font.family: parent.modelData.length > 0 ? "monospace" : Qt.application.font.family
                        font.pixelSize: Theme.subheadSize
                        horizontalAlignment: TextEdit.AlignRight
                        wrapMode: TextEdit.WrapAnywhere
                        readOnly: true
                        selectByMouse: true
                        selectionColor: Theme.accent
                        selectedTextColor: "white"
                    }
                }
            }
        }

        SettingsNote {
            text: L10n.t("Phone Link registers this computer with your Microsoft account as a Windows device so your phone can reach it.")
        }
    }

    SettingsSection {
        title: L10n.t("Reset")

        PillButton {
            enabled: pane.account.canSignOut === true
            destructive: true
            text: L10n.t("Reset Device Registration...")
            onClicked: confirm.open()
        }

        SettingsNote {
            text: L10n.t("Removes this computer's records from your account and signs out. The next sign-in registers this computer again with a new ID. The earlier registration stays on your Microsoft account.")
        }
    }

    LogSection {}

    MessageDialog {
        id: confirm
        heading: L10n.t("Reset This Computer's Device Registration?")
        message: L10n.t("This computer will be removed from your account and signed out. Sign in again to reconnect your phone.")
        acceptText: L10n.t("Reset")
        destructive: true
        onAccepted: {
            EngineBridge.perform("account.resetRegistration", "")
            pane.signedOut()
        }
    }
}
