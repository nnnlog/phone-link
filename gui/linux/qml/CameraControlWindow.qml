import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import QtQuick.Window
import PhoneLink

// The camera control window: shown while an app uses the phone's camera. It shows the connection,
// the controls while the video runs (pause, switch cameras, effects), the phone's battery and a
// poor-network warning, and what to do when something stops the video.
Window {
    id: window

    property var camera: null

    readonly property string state: camera ? camera.state : ""
    readonly property var errorText: texts(camera ? camera.error : "None")
    readonly property var effects: camera && camera.effects ? camera.effects : []
    // The phone asks for notifications to be allowed, with a code to scan.
    readonly property string qrCode: {
        if (!camera || camera.error !== "PostNotificationPermissionNeeded" || !camera.qrCodeUri)
            return ""
        return JSON.parse(EngineBridge.query("qr", camera.qrCodeUri)) || ""
    }

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

    // A failed action, while this is the active window.
    function showFailure(title, message) {
        failureToast.show(title, message)
    }

    title: EngineBridge.phoneName.length > 0 ? L10n.f("%@ Camera", [EngineBridge.phoneName]) : L10n.t("Phone Camera")
    width: 430
    // A failure shows under the controls, the window growing for it.
    height: Math.max(160, column.implicitHeight + 32 + (failureToast.visible ? failureToast.implicitHeight + 12 : 0))
    minimumWidth: 430
    maximumWidth: 430
    flags: Qt.Window | Qt.WindowStaysOnTopHint
    color: Theme.windowBackground
    visible: true

    palette.window: Theme.windowBackground
    palette.windowText: Theme.text
    palette.base: Theme.fieldBackground
    palette.text: Theme.text
    palette.button: Theme.controlBackground
    palette.buttonText: Theme.text
    palette.highlight: Theme.accent
    palette.highlightedText: "white"
    palette.placeholderText: Theme.secondaryText
    palette.light: Theme.paletteLight
    palette.midlight: Theme.paletteMidlight
    palette.mid: Theme.paletteMid
    palette.dark: Theme.paletteDark
    palette.shadow: "black"
    palette.toolTipBase: Theme.toolTipBackground
    palette.toolTipText: Theme.text

    onClosing: close => {
        close.accepted = true
        EngineBridge.perform("camera.closeControlWindow", "")
    }

    ColumnLayout {
        id: column
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.top: parent.top
        anchors.margins: 16
        spacing: 12

        Text {
            Layout.fillWidth: true
            visible: window.camera === null
            horizontalAlignment: Text.AlignHCenter
            text: L10n.t("Phone Link isn't connected.")
            color: Theme.secondaryText
            font.pixelSize: Theme.bodySize
            wrapMode: Text.WordWrap
        }

        ColumnLayout {
            Layout.fillWidth: true
            visible: window.state === "Connecting"
            spacing: 8

            BusyIndicator {
                Layout.alignment: Qt.AlignHCenter
                running: parent.visible
            }

            Text {
                Layout.fillWidth: true
                horizontalAlignment: Text.AlignHCenter
                text: L10n.t("Connecting to your phone...")
                color: Theme.text
                font.pixelSize: Theme.bodySize
                wrapMode: Text.WordWrap
            }
        }

        // The video runs (or is paused): the camera in use and the phone's battery on the first line,
        // then Pause or Resume, Switch Camera and the effect picker on one line.
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
                    Accessible.ignored: true
                }

                Text {
                    Layout.fillWidth: true
                    text: window.title
                    color: Theme.text
                    font.pixelSize: Theme.headlineSize
                    font.weight: Font.DemiBold
                    elide: Text.ElideRight
                }

                Row {
                    visible: window.camera !== null && window.camera.batteryLevel >= 0
                    spacing: 3
                    Accessible.role: Accessible.StaticText
                    Accessible.name: L10n.f("Battery %d%%", [window.camera ? window.camera.batteryLevel : 0])

                    Text {
                        anchors.verticalCenter: parent.verticalCenter
                        text: window.camera ? window.camera.batteryLevel + "%" : ""
                        color: Theme.secondaryText
                        font.pixelSize: Theme.subheadSize
                    }

                    Icon {
                        anchors.verticalCenter: parent.verticalCenter
                        name: window.camera && window.camera.charging ? "batteryCharging" : "battery"
                        size: 18
                        color: Theme.secondaryText
                    }
                }
            }

            RowLayout {
                visible: window.camera !== null && window.camera.lowNetwork === true
                spacing: 6

                Icon {
                    name: "wifiWarning"
                    size: 16
                    color: Theme.warning
                }

                Text {
                    text: L10n.t("Poor connection")
                    color: Theme.warning
                    font.pixelSize: Theme.bodySize
                }
            }

            RowLayout {
                Layout.fillWidth: true
                spacing: 8

                PillButton {
                    text: window.state === "Paused" ? L10n.t("Resume") : L10n.t("Pause")
                    onClicked: EngineBridge.perform(window.state === "Paused" ? "camera.resume" : "camera.pause", "")
                }

                PillButton {
                    text: L10n.t("Switch Camera")
                    enabled: window.camera !== null && window.camera.face !== null
                    onClicked: EngineBridge.perform("camera.switch", window.camera.face === "Front" ? "Back" : "Front")
                }

                Item {
                    Layout.fillWidth: true
                }

                CameraEffectPicker {
                    visible: window.effects.length > 1
                    Layout.maximumWidth: 180
                    model: window.effects
                    selectedIndex: window.camera && window.camera.selectedEffect !== null ? window.camera.selectedEffect : 0
                    onPicked: index => EngineBridge.perform("camera.effect", String(index))
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
            spacing: 8

            Icon {
                Layout.alignment: Qt.AlignHCenter
                name: "handTap"
                size: 36
                color: Theme.secondaryText
            }

            Text {
                Layout.fillWidth: true
                horizontalAlignment: Text.AlignHCenter
                text: L10n.t("Tap the notification on your phone")
                color: Theme.text
                font.pixelSize: Theme.headlineSize
                font.weight: Font.DemiBold
                wrapMode: Text.WordWrap
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
            spacing: 8

            Icon {
                Layout.alignment: Qt.AlignHCenter
                name: "warning"
                size: 36
                color: Theme.secondaryText
            }

            Text {
                Layout.fillWidth: true
                horizontalAlignment: Text.AlignHCenter
                text: window.errorText[0]
                color: Theme.text
                font.pixelSize: Theme.headlineSize
                font.weight: Font.DemiBold
                wrapMode: Text.WordWrap
            }

            Text {
                Layout.fillWidth: true
                horizontalAlignment: Text.AlignHCenter
                text: window.errorText[1]
                color: Theme.secondaryText
                font.pixelSize: Theme.bodySize
                wrapMode: Text.WordWrap
            }

            // On white, so the code can be scanned in either theme.
            Rectangle {
                Layout.alignment: Qt.AlignHCenter
                Layout.topMargin: 4
                visible: window.qrCode.length > 0
                width: 172
                height: 172
                radius: 6
                color: "white"

                Image {
                    anchors.centerIn: parent
                    width: 160
                    height: 160
                    source: window.qrCode
                    smooth: false
                    fillMode: Image.PreserveAspectFit
                    Accessible.role: Accessible.Graphic
                    Accessible.name: L10n.t("Scan the code with your phone to allow Link to Windows to show notifications, then reconnect.")
                }
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
            Layout.fillWidth: true
            horizontalAlignment: Text.AlignHCenter
            visible: window.state === "Registered" || window.state === "Unregistered"
            text: L10n.t("The camera isn't in use.")
            color: Theme.secondaryText
            font.pixelSize: Theme.bodySize
            wrapMode: Text.WordWrap
        }
    }

    MirrorFailureToast {
        id: failureToast
        anchors.horizontalCenter: parent.horizontalCenter
        anchors.bottom: parent.bottom
        anchors.bottomMargin: 12
    }
}
