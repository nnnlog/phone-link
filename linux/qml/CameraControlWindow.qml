import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import QtQuick.Window
import PhoneLink

Window {
    id: window

    property var camera: null

    readonly property string phoneName: EngineBridge.phoneName.length > 0 ? EngineBridge.phoneName : L10n.t("Phone")
    readonly property string state: camera ? camera.state : ""
    readonly property var errorText: texts(camera ? camera.error : "None")

    function texts(error) {
        switch (error) {
        case "ChannelDisconnected":
        case "Disconnected":
            return [L10n.t("Your phone is disconnected"), L10n.t("Make sure your phone is nearby and connected, then reconnect.")]
        case "MeteredConnection":
            return [L10n.t("Your phone is on mobile data"), L10n.t("Link to Windows doesn't use mobile data on your phone. Connect it to Wi-Fi or allow mobile data, then reconnect.")]
        case "LtwDisabled":
            return [L10n.t("Link to Windows is turned off"), L10n.t("Turn on Link to Windows on your phone, then reconnect.")]
        case "PostNotificationPermissionNeeded":
            return [L10n.t("Allow notifications on your phone"), L10n.t("Scan the code with your phone to allow Link to Windows to show notifications, then reconnect.")]
        case "CameraPermissionNeeded":
        case "CameraPermissionNeededAutoprompt":
            return [L10n.t("Allow camera access on your phone"), L10n.t("Link to Windows needs your phone's camera. Allow it on your phone.")]
        case "CameraPermissionDenied":
            return [L10n.t("Camera access was denied"), L10n.t("Allow Link to Windows to use the camera on your phone, then try again.")]
        case "CameraPermissionPermanentlyDenied":
            return [L10n.t("Camera access is turned off"), L10n.t("Allow camera access for Link to Windows in your phone's settings, then try again.")]
        case "CameraPermissionRevoked":
            return [L10n.t("Camera access was removed"), L10n.t("Allow camera access for Link to Windows on your phone.")]
        case "ScreenLocked":
            return [L10n.t("Your phone is locked"), L10n.t("Unlock your phone to continue.")]
        case "CallInProgress":
            return [L10n.t("A call is in progress"), L10n.t("The camera continues after the call.")]
        case "DeviceHeat":
            return [L10n.t("Your phone is too warm"), L10n.t("Let your phone cool down, then try again.")]
        case "CameraInUse":
        case "MaxCamerasInUse":
            return [L10n.t("Your phone's camera is in use"), L10n.t("Close the other app using the camera on your phone.")]
        case "CameraDeviceFatal":
        case "CameraServiceFatal":
        case "SessionConfigurationFailed":
            return [L10n.t("Your phone's camera stopped"), L10n.t("Restart the camera on your phone, then try again.")]
        case "CameraDisabled":
        case "NoCameraAvailable":
            return [L10n.t("Your phone's camera isn't available"), L10n.t("Check that the camera works on your phone.")]
        case "ChannelNotCreated":
            return [L10n.t("Unlock this computer"), L10n.t("The camera connects once this computer is unlocked.")]
        default:
            return [L10n.t("Something went wrong"), L10n.t("Try again.")]
        }
    }

    function errorAction() {
        switch (camera ? camera.errorAction : "") {
        case "Refresh": EngineBridge.perform("camera.refresh", ""); break
        case "RequestPermissions": EngineBridge.perform("camera.requestPermissions", ""); break
        case "Close": EngineBridge.perform("camera.closeError", ""); break
        }
    }

    title: L10n.f("%@ Camera", [phoneName])
    width: 430
    height: Math.max(160, column.implicitHeight + 32)
    minimumWidth: 430
    maximumWidth: 430
    flags: Qt.Window | Qt.WindowStaysOnTopHint
    color: Theme.windowBackground
    visible: true

    onClosing: close => {
        close.accepted = true
        EngineBridge.perform("camera.closeControlWindow", "")
    }

    ColumnLayout {
        id: column
        anchors.fill: parent
        anchors.margins: 16
        spacing: 12

        ColumnLayout {
            Layout.fillWidth: true
            visible: window.state === "Connecting"
            spacing: 8

            BusyIndicator {
                Layout.alignment: Qt.AlignHCenter
                running: parent.visible
            }

            Text {
                Layout.alignment: Qt.AlignHCenter
                text: L10n.t("Connecting to your phone...")
                color: Theme.text
                font.pixelSize: Theme.bodySize
            }
        }

        ColumnLayout {
            Layout.fillWidth: true
            visible: window.state === "Streaming" || window.state === "Paused"
            spacing: 12

            RowLayout {
                Layout.fillWidth: true
                spacing: 8

                Rectangle {
                    width: 8
                    height: 8
                    radius: 4
                    color: "#34c759"
                }

                Text {
                    Layout.fillWidth: true
                    text: L10n.f("%@ Camera", [window.phoneName])
                    color: Theme.text
                    font.pixelSize: Theme.headlineSize
                    font.weight: Font.DemiBold
                    elide: Text.ElideRight
                }

                Text {
                    visible: window.camera !== null && window.camera.batteryLevel >= 0
                    text: window.camera ? window.camera.batteryLevel + "%" : ""
                    color: Theme.secondaryText
                    font.pixelSize: Theme.subheadSize
                }
            }

            Text {
                visible: window.camera !== null && window.camera.lowNetwork === true
                text: L10n.t("Poor connection")
                color: Theme.warning
                font.pixelSize: Theme.bodySize
            }

            RowLayout {
                spacing: 8

                PillButton {
                    text: L10n.t(window.state === "Paused" ? "Resume" : "Pause")
                    onClicked: EngineBridge.perform(window.state === "Paused" ? "camera.resume" : "camera.pause", "")
                }

                PillButton {
                    text: L10n.t("Switch Camera")
                    enabled: window.camera !== null && window.camera.face !== null
                    onClicked: EngineBridge.perform("camera.switch", window.camera.face === "Front" ? "Back" : "Front")
                }
            }

            Text {
                Layout.fillWidth: true
                visible: window.state === "Paused" && window.camera !== null && window.camera.error !== "None"
                text: window.errorText[1]
                color: Theme.secondaryText
                font.pixelSize: Theme.bodySize
                wrapMode: Text.WordWrap
            }
        }

        ColumnLayout {
            Layout.fillWidth: true
            visible: window.state === "StartStreamNotification"
            spacing: 6

            Text {
                Layout.alignment: Qt.AlignHCenter
                text: L10n.t("Tap the notification on your phone")
                color: Theme.text
                font.pixelSize: Theme.headlineSize
                font.weight: Font.DemiBold
            }

            Text {
                Layout.fillWidth: true
                horizontalAlignment: Text.AlignHCenter
                text: L10n.t("Your phone shows a notification from Link to Windows. Tap it to start the camera.")
                color: Theme.secondaryText
                font.pixelSize: Theme.bodySize
                wrapMode: Text.WordWrap
            }
        }

        ColumnLayout {
            Layout.fillWidth: true
            visible: window.state === "Error" || window.state === "TerminalError"
            spacing: 6

            Icon {
                Layout.alignment: Qt.AlignHCenter
                name: "warning"
                size: 36
                color: Theme.secondaryText
            }

            Text {
                Layout.alignment: Qt.AlignHCenter
                text: window.errorText[0]
                color: Theme.text
                font.pixelSize: Theme.headlineSize
                font.weight: Font.DemiBold
            }

            Text {
                Layout.fillWidth: true
                horizontalAlignment: Text.AlignHCenter
                text: window.errorText[1]
                color: Theme.secondaryText
                font.pixelSize: Theme.bodySize
                wrapMode: Text.WordWrap
            }

            PillButton {
                Layout.alignment: Qt.AlignHCenter
                Layout.topMargin: 6
                readonly property string errorAction: window.camera ? window.camera.errorAction : "None"
                visible: errorAction !== "None"
                prominent: true
                text: errorAction === "Refresh" ? L10n.t("Reconnect")
                    : errorAction === "RequestPermissions" ? L10n.t("Send Request to Phone") : L10n.t("Close")
                onClicked: window.errorAction()
            }
        }

        Text {
            Layout.alignment: Qt.AlignHCenter
            visible: window.state === "Registered" || window.state === "Unregistered"
            text: L10n.t("The camera isn't in use.")
            color: Theme.secondaryText
            font.pixelSize: Theme.bodySize
        }
    }
}
