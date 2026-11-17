import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import PhoneLink

// Photos: whether the phone's recent photos show on this computer, what may be done with them, and
// the notifications the phone sends when a photo is taken.
SettingsPane {
    id: pane

    readonly property var photos: {
        EngineBridge.settingsRevision
        EngineBridge.photosRevision
        return JSON.parse(EngineBridge.query("photos.state", ""))
    }
    readonly property var newPhotos: { EngineBridge.settingsRevision; return JSON.parse(EngineBridge.query("snipping.settings", "")) }
    // The engine runs, so the settings can be read and changed.
    readonly property bool connected: photos !== null && photos.content !== "nothing"
    readonly property bool consent: connected && photos.consent === true

    SettingsSection {
        title: L10n.t("Photos")

        SettingsNote {
            visible: !pane.connected
            text: L10n.t("Settings are available once Phone Link is connected.")
        }

        MacSwitch {
            Layout.fillWidth: true
            text: L10n.t("Show photos on this computer")
            enabled: pane.connected
            checked: pane.consent
            onToggled: EngineBridge.perform("photos.setConsent", String(checked))
        }

        // A phone that sends its own new-photo notifications has the switch below instead.
        MacSwitch {
            Layout.fillWidth: true
            visible: pane.newPhotos === null || pane.newPhotos.coversPhotosNotifications !== true
            text: L10n.t("Notify me about new photos")
            enabled: pane.consent
            checked: pane.connected && pane.photos.newPhotoNotifications === true
            onToggled: EngineBridge.perform("photos.setNotifications", String(checked))
        }

        MacSwitch {
            Layout.fillWidth: true
            text: L10n.t("Allow deleting photos from this computer")
            enabled: pane.consent
            checked: pane.connected && pane.photos.deletionAllowed === true
            onToggled: EngineBridge.perform("photos.setDeletionAllowed", String(checked))
        }
    }

    SettingsSection {
        title: L10n.t("New Photos")
        visible: pane.newPhotos !== null && pane.newPhotos.applicable === true

        MacSwitch {
            Layout.fillWidth: true
            text: L10n.t("Notify me when I take a photo on my phone")
            checked: pane.newPhotos !== null && pane.newPhotos.enabled === true
            onToggled: EngineBridge.perform("snipping.setEnabled", String(checked))
        }

        SettingsNote {
            text: L10n.t("Click the notification to open or edit the photo on this computer")
        }

        // The phone has not allowed photo access for new-photo notifications: a button that asks it.
        RowLayout {
            Layout.fillWidth: true
            visible: pane.newPhotos !== null && pane.newPhotos.needsPermission === true
            spacing: 8

            Text {
                Layout.fillWidth: true
                text: L10n.t("Your phone needs to allow access to photos")
                color: Theme.text
                font.pixelSize: Theme.bodySize
                wrapMode: Text.Wrap
            }

            BusyIndicator {
                Layout.preferredWidth: 20
                Layout.preferredHeight: 20
                padding: 0
                visible: pane.newPhotos !== null && pane.newPhotos.requestingPermission === true
                running: visible
            }

            Text {
                visible: pane.newPhotos !== null && pane.newPhotos.requestingPermission === true
                text: L10n.t("Waiting for your phone...")
                color: Theme.secondaryText
                font.pixelSize: Theme.bodySize
            }

            PillButton {
                visible: pane.newPhotos === null || pane.newPhotos.requestingPermission !== true
                text: L10n.t("Send Request to Phone")
                onClicked: EngineBridge.perform("snipping.requestPermission", "")
            }
        }
    }
}
