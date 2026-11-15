import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import PhoneLink

ApplicationWindow {
    id: window

    readonly property var settings: { EngineBridge.settingsRevision; return JSON.parse(EngineBridge.query("settings", "")) || {} }
    readonly property var phoneFiles: { EngineBridge.settingsRevision; return JSON.parse(EngineBridge.query("phoneFiles", "")) }
    readonly property var camera: { EngineBridge.settingsRevision; return JSON.parse(EngineBridge.query("camera", "")) }

    title: L10n.t("Settings")
    width: 560
    height: 520
    minimumWidth: 480
    minimumHeight: 420
    color: Theme.windowBackground

    component Section: Rectangle {
        default property alias rows: column.data
        property string title

        Layout.fillWidth: true
        implicitHeight: column.implicitHeight + 32
        radius: Theme.moduleRadius
        color: Theme.listBackground
        border.color: Theme.separator

        Text {
            x: 4
            y: -20
            text: parent.title
            color: Theme.secondaryText
            font.pixelSize: Theme.subheadSize
            font.weight: Font.DemiBold
        }

        ColumnLayout {
            id: column
            anchors.fill: parent
            anchors.margins: 16
            spacing: 12
        }
    }

    component Note: Text {
        Layout.fillWidth: true
        color: Theme.secondaryText
        font.pixelSize: Theme.captionSize
        wrapMode: Text.Wrap
    }

    Flickable {
        anchors.fill: parent
        contentHeight: content.implicitHeight + 60
        clip: true
        ScrollBar.vertical: ScrollBar {}

        ColumnLayout {
            id: content
            x: 24
            y: 40
            width: parent.width - 48
            spacing: 40

            Section {
                title: L10n.t("General")

                MacSwitch {
                    Layout.fillWidth: true
                    text: L10n.t("Share clipboard")
                    enabled: window.settings.clipboardApplies === true
                    checked: window.settings.clipboard === true
                    onToggled: EngineBridge.perform("settings.clipboard", String(checked))
                }

                Note {
                    text: window.settings.clipboardApplies === true
                          ? L10n.t("Text and images you copy on one device can be pasted on the other.")
                          : L10n.t("This phone doesn't support clipboard sharing.")
                }

                MacSwitch {
                    Layout.fillWidth: true
                    text: L10n.t("Allow the phone to control this computer")
                    enabled: window.settings.remoteControlApplies === true
                    checked: window.settings.remoteControl === true
                    onToggled: EngineBridge.perform("settings.remoteControl", String(checked))
                }

                Note {
                    text: L10n.t("Your phone can lock, sleep, restart or shut down this computer.")
                }

                MacSwitch {
                    Layout.fillWidth: true
                    visible: window.settings.fileSharingShown === true
                    text: L10n.t("Receive files from your phone")
                    checked: window.settings.fileSharing === true
                    onToggled: EngineBridge.perform("settings.fileSharing", String(checked))
                }
            }

            Section {
                title: L10n.t("Phone Files")
                visible: window.phoneFiles !== null && window.phoneFiles.capable === true

                MacSwitch {
                    Layout.fillWidth: true
                    text: L10n.t("Show phone files in the file manager")
                    checked: window.phoneFiles !== null && window.phoneFiles.enabled === true
                    onToggled: EngineBridge.perform("phoneFiles.enable", String(checked))
                }

                Note {
                    text: window.phoneFiles !== null && window.phoneFiles.permission !== true
                          ? L10n.t("Allow access to files on your phone to see them here.")
                          : L10n.f("Your phone's files appear in %@.", [window.phoneFiles ? window.phoneFiles.path : ""])
                }

                RowLayout {
                    spacing: 8

                    PillButton {
                        visible: window.phoneFiles !== null && window.phoneFiles.enabled === true
                                 && window.phoneFiles.permission !== true
                        text: L10n.t("Send Request to Phone")
                        onClicked: EngineBridge.perform("phoneFiles.requestPermission", "")
                    }

                    PillButton {
                        visible: window.phoneFiles !== null && window.phoneFiles.running === true
                                 && window.phoneFiles.permission === true
                        text: L10n.t("Open Phone Files")
                        onClicked: Qt.openUrlExternally("file://" + window.phoneFiles.path)
                    }
                }
            }

            Section {
                title: L10n.t("Camera")
                visible: window.camera !== null && window.camera.supported === true

                MacSwitch {
                    Layout.fillWidth: true
                    text: L10n.t("Use as a camera")
                    enabled: window.camera !== null && window.camera.available === true
                    checked: window.camera !== null && window.camera.enabled === true
                    onToggled: EngineBridge.perform("camera.enable", String(checked))
                }

                Note {
                    text: window.camera !== null && window.camera.available === true
                          ? L10n.t("Apps on this computer can use your phone's camera.")
                          : L10n.t("Load the v4l2loopback kernel module to use your phone as a camera.")
                }
            }

            Section {
                title: L10n.t("Calls")
                visible: window.settings.callsOffered === true

                MacSwitch {
                    Layout.fillWidth: true
                    text: L10n.t("Place and answer calls from this computer")
                    checked: window.settings.callsEnabled === true
                    onToggled: EngineBridge.perform("calls.enable", String(checked))
                }

                Note {
                    text: L10n.t("The call stays on your phone: you talk on the phone or a headset connected to it.")
                }

                RowLayout {
                    visible: window.settings.callsEnabled === true
                    spacing: 8

                    PillButton {
                        text: L10n.t("Pair with Phone")
                        onClicked: EngineBridge.perform("calls.pair", "")
                    }

                    PillButton {
                        text: L10n.t("Reconnect")
                        onClicked: EngineBridge.perform("calls.connect", "")
                    }
                }
            }

            Section {
                title: L10n.t("Appearance")

                MacSwitch {
                    Layout.fillWidth: true
                    text: L10n.t("Use phone wallpaper as sidebar background")
                    checked: Preferences.wallpaperBackground
                    onToggled: Preferences.wallpaperBackground = checked
                }
            }

            Section {
                title: L10n.t("Microsoft Account")

                Note {
                    text: L10n.t("This computer will stop connecting to your phone, and its copies of your messages, photos, contacts and notifications will be deleted.")
                }

                PillButton {
                    text: L10n.t("Sign Out")
                    onClicked: confirm.open()
                }
            }
        }
    }

    Dialog {
        id: confirm
        anchors.centerIn: parent
        modal: true
        title: L10n.t("Sign Out")
        standardButtons: Dialog.Ok | Dialog.Cancel
        onAccepted: {
            EngineBridge.perform("signOut", "")
            window.close()
        }

        Text {
            width: 320
            text: L10n.t("This computer will stop connecting to your phone, and its copies of your messages, photos, contacts and notifications will be deleted.")
            color: Theme.text
            wrapMode: Text.Wrap
        }
    }
}
