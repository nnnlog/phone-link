import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import PhoneLink

// Camera: the phone as a camera on this computer — the v4l2loopback device it is written to, the
// switch, the phone's camera permission, and a failure to publish the camera with its retry.
SettingsPane {
    id: pane

    readonly property var camera: { EngineBridge.settingsRevision; return JSON.parse(EngineBridge.query("camera", "")) }
    readonly property var device: { EngineBridge.settingsRevision; return JSON.parse(EngineBridge.query("camera.device", "")) }
    readonly property bool supported: camera !== null && camera.supported === true

    SettingsSection {
        title: L10n.t("Camera")

        RowLayout {
            Layout.fillWidth: true
            spacing: 12

            Text {
                Layout.fillWidth: true
                text: L10n.t("Camera device")
                color: Theme.text
                font.pixelSize: Theme.bodySize
                wrapMode: Text.Wrap
            }

            Text {
                text: pane.device !== null && pane.device.path ? pane.device.path : L10n.t("Not set up")
                color: Theme.secondaryText
                font.pixelSize: Theme.bodySize
            }
        }

        SettingsNote {
            text: {
                const problem = pane.device !== null ? pane.device.problem : null
                if (problem === "noLoopback")
                    return L10n.t("Load the v4l2loopback kernel module to use your phone as a camera.")
                if (problem === "unlabeled")
                    return L10n.t("Several v4l2loopback devices are loaded, and none is labeled “Phone Link”. Give one that label with its card_label option.")
                return L10n.t("Phone Link adds your phone to this computer's cameras through a v4l2loopback device labeled “Phone Link”.")
            }
        }

        // The command that loads the device, to copy into a terminal.
        TextEdit {
            Layout.fillWidth: true
            visible: pane.device !== null && pane.device.problem === "noLoopback"
            readOnly: true
            selectByMouse: true
            wrapMode: TextEdit.Wrap
            text: "sudo modprobe v4l2loopback card_label=\"Phone Link\" exclusive_caps=1"
            color: Theme.text
            selectionColor: Theme.accent
            selectedTextColor: "white"
            font.family: "monospace"
            font.pixelSize: Theme.captionSize
        }
    }

    SettingsSection {
        Text {
            Layout.fillWidth: true
            visible: !pane.supported
            text: pane.camera === null ? L10n.t("Settings are available once Phone Link is connected.")
                                       : L10n.t("This phone can't be used as a camera.")
            color: Theme.secondaryText
            font.pixelSize: Theme.bodySize
            wrapMode: Text.Wrap
        }

        MacSwitch {
            Layout.fillWidth: true
            visible: pane.supported
            text: L10n.t("Use your phone as a camera")
            enabled: pane.camera !== null && pane.camera.available === true
            checked: pane.camera !== null && pane.camera.enabled === true
            onToggled: EngineBridge.perform("camera.enable", String(checked))
        }

        SettingsNote {
            Layout.topMargin: -8
            visible: pane.supported
            text: L10n.t("Choose it as the camera in any app on this computer")
        }

        // Access the phone gives this computer: "Allowed", or a request for it on the phone.
        RowLayout {
            Layout.fillWidth: true
            visible: pane.supported && pane.camera.enabled === true
            spacing: 12

            // Lined up with the switch's text above it.
            Text {
                Layout.fillWidth: true
                leftPadding: 6
                text: L10n.t("Camera access on the phone")
                color: Theme.text
                font.pixelSize: Theme.bodySize
                wrapMode: Text.Wrap
            }

            Text {
                visible: pane.camera !== null && pane.camera.cameraPermission === true
                text: L10n.t("Allowed")
                color: Theme.secondaryText
                font.pixelSize: Theme.bodySize
            }

            BusyIndicator {
                visible: pane.camera !== null && pane.camera.cameraPermission !== true
                         && pane.camera.requestingPermission === true
                running: visible
                padding: 0
                implicitWidth: 24
                implicitHeight: 24
                Accessible.name: L10n.t("Waiting for your phone")
            }

            PillButton {
                visible: pane.camera !== null && pane.camera.cameraPermission !== true
                         && pane.camera.requestingPermission !== true
                text: L10n.t("Send Request to Phone")
                onClicked: EngineBridge.perform("camera.requestPermission", "")
            }
        }

        RowLayout {
            Layout.fillWidth: true
            visible: pane.supported && pane.camera.registrationFailed === true
            spacing: 8

            Icon {
                Layout.alignment: Qt.AlignTop
                Layout.topMargin: 2
                name: "warning"
                size: 16
                color: Theme.warning
            }

            Text {
                Layout.fillWidth: true
                text: L10n.t("Your phone couldn't be added as a camera.")
                color: Theme.text
                font.pixelSize: Theme.bodySize
                wrapMode: Text.Wrap
            }

            PillButton {
                text: L10n.t("Try Again")
                onClicked: EngineBridge.perform("camera.retry", "")
            }
        }

        SettingsNote {
            visible: pane.supported && pane.camera.replacedPhone !== null && pane.camera.replacedPhone !== undefined
            text: L10n.f("%@ is no longer used as a camera. Only one phone can be a camera at a time.",
                         [pane.camera && pane.camera.replacedPhone ? pane.camera.replacedPhone : ""])
        }
    }
}
