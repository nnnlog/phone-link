import QtQuick
import QtQuick.Layouts
import PhoneLink

// Account: the signed-in Microsoft account and signing out, the account's linked phones (which one this
// computer uses, and pairing another), and the account's PCs.
SettingsPane {
    id: pane

    signal signedOut()

    readonly property var account: {
        EngineBridge.settingsRevision
        EngineBridge.phase
        return JSON.parse(EngineBridge.query("shell.account", "")) || {}
    }
    readonly property var phones: {
        EngineBridge.phonesJson
        EngineBridge.pairingRevision
        EngineBridge.phoneName
        return JSON.parse(EngineBridge.query("shell.phones", "")) || []
    }

    // The phone's model (when its name is not already the model) and when the account last saw it.
    function detail(phone) {
        const parts = []
        if (phone.model)
            parts.push(phone.model)
        if (phone.lastSeenMs)
            parts.push(L10n.f("Last seen %@", [Dates.listStamp(phone.lastSeenMs)]))
        return parts.join(" · ")
    }

    // On screen: the current tab of an open Settings window.
    readonly property bool shown: visible && Window.window !== null && Window.window.visible

    // The linked phones and the account's name are read again each time the pane shows.
    onShownChanged: {
        if (shown) {
            EngineBridge.perform("shell.reloadPhones", "")
            EngineBridge.perform("account.loadName", "")
        }
    }

    SettingsNote {
        visible: EngineBridge.phase !== "running"
        text: L10n.t("Settings are available once Phone Link is connected.")
    }

    SettingsSection {
        title: L10n.t("Microsoft Account")

        RowLayout {
            Layout.fillWidth: true
            spacing: 12

            ColumnLayout {
                Layout.fillWidth: true
                spacing: 2

                Text {
                    Layout.fillWidth: true
                    text: pane.account.name || L10n.t("Microsoft account")
                    color: Theme.text
                    font.pixelSize: Theme.bodySize
                    elide: Text.ElideRight
                }

                Text {
                    Layout.fillWidth: true
                    text: pane.account.signedIn === false ? L10n.t("Signed out") : L10n.t("Signed in")
                    color: Theme.secondaryText
                    font.pixelSize: Theme.captionSize
                }
            }

            PillButton {
                enabled: pane.account.canSignOut === true
                text: L10n.t("Sign Out...")
                onClicked: confirm.open()
            }
        }
    }

    SettingsSection {
        title: L10n.t("Phones")

        Repeater {
            model: pane.phones

            delegate: RowLayout {
                id: row

                required property var modelData

                Layout.fillWidth: true
                spacing: 12

                ColumnLayout {
                    Layout.fillWidth: true
                    spacing: 2

                    Text {
                        Layout.fillWidth: true
                        text: row.modelData.name
                        color: Theme.text
                        font.pixelSize: Theme.bodySize
                        elide: Text.ElideRight
                    }

                    Text {
                        Layout.fillWidth: true
                        visible: text.length > 0
                        text: pane.detail(row.modelData)
                        color: Theme.secondaryText
                        font.pixelSize: Theme.captionSize
                        elide: Text.ElideRight
                    }
                }

                Text {
                    visible: row.modelData.inUse === true
                    text: L10n.t("In Use")
                    color: Theme.secondaryText
                    font.pixelSize: Theme.bodySize
                }

                // Switches to this phone; the previous phone is disconnected.
                PillButton {
                    visible: row.modelData.inUse !== true
                    enabled: EngineBridge.phase === "running" || EngineBridge.phase === "selectingPhone"
                    text: L10n.t("Use This Phone")
                    onClicked: EngineBridge.selectPhone(row.modelData.id)
                }
            }
        }

        PillButton {
            Layout.alignment: Qt.AlignRight
            text: L10n.t("Pair a New Phone...")
            onClicked: EngineBridge.perform("shell.showPairing", "")
        }
    }

    AccountPcsSection {}

    MessageDialog {
        id: confirm
        heading: L10n.t("Sign Out of Phone Link?")
        message: L10n.t("This computer will stop connecting to your phone, and its copies of your messages, photos, contacts and notifications will be deleted. Your phone stays linked to your Microsoft account.")
        acceptText: L10n.t("Sign Out")
        destructive: true
        onAccepted: {
            EngineBridge.perform("signOut", "")
            pane.signedOut()
        }
    }
}
