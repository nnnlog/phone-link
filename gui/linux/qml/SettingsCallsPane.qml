import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import PhoneLink

// Calls: the phone's call history on this computer, and placing and answering the phone's calls from
// this computer over Bluetooth, with the calls themselves on the phone. Until the engine runs, and for a
// phone that can't make calls, notes stand in for the controls.
SettingsPane {
    id: pane

    readonly property var history: { EngineBridge.callsRevision; EngineBridge.settingsRevision; return JSON.parse(EngineBridge.query("callsState", "")) }
    readonly property var control: { EngineBridge.callsRevision; EngineBridge.settingsRevision; return JSON.parse(EngineBridge.query("callControl", "")) || {} }
    readonly property bool ready: EngineBridge.phase === "running" && history !== null
    readonly property bool supported: history !== null && history.access !== "unsupported"
    readonly property bool hasHistory: history !== null && history.pcConsent === true
    readonly property string statusText: {
        switch (control.status) {
        case "starting":
        case "connecting": return L10n.t("Connecting...")
        case "bluetoothOff": return L10n.t("Bluetooth is off")
        case "notPaired": return L10n.t("Not paired")
        case "connectionFailed": return L10n.t("Couldn't connect")
        case "notConnected": return L10n.t("Not connected")
        case "ready": return L10n.t("Connected")
        default: return L10n.t("Off")
        }
    }

    // Microsoft's page on emergency calls with Phone Link.
    readonly property string emergencyInfo: "https://go.microsoft.com/fwlink/?linkid=2111247"

    SettingsSection {
        title: L10n.t("Calls")
        visible: !pane.ready || !pane.supported

        SettingsNote {
            visible: !pane.ready
            text: L10n.t("Settings are available once Phone Link is connected.")
        }

        SettingsNote {
            text: L10n.t("Calls need an Android phone that can make phone calls.")
        }
    }

    SettingsSection {
        title: L10n.t("Call History")
        visible: pane.ready && pane.supported

        MacSwitch {
            Layout.fillWidth: true
            text: L10n.t("Show call history on this computer")
            checked: pane.hasHistory
            onToggled: EngineBridge.perform("calls.setHistory", String(checked))
        }

        // Only once the phone shares call history and nothing but its permission is left.
        RowLayout {
            Layout.fillWidth: true
            visible: pane.history !== null && (pane.history.access === "permissionRequired" || pane.history.access === "granted")
            spacing: 8

            Text {
                Layout.fillWidth: true
                text: L10n.t("Call history access")
                color: Theme.text
                font.pixelSize: Theme.bodySize
                wrapMode: Text.Wrap
            }

            Text {
                visible: pane.history !== null && pane.history.access === "granted"
                text: L10n.t("Allowed")
                color: Theme.secondaryText
                font.pixelSize: Theme.bodySize
            }

            BusyIndicator {
                visible: pane.history !== null && pane.history.access !== "granted" && pane.history.accessPending === true
                running: visible
                padding: 1
                implicitWidth: 20
                implicitHeight: 20
                Accessible.name: L10n.t("Waiting for your phone")
            }

            PillButton {
                visible: pane.history !== null && pane.history.access !== "granted" && pane.history.accessPending !== true
                text: L10n.t("Send Request to Phone")
                onClicked: EngineBridge.perform("calls.requestAccess", "")
            }
        }
    }

    SettingsSection {
        title: L10n.t("Calls")
        visible: pane.ready && pane.supported

        MacSwitch {
            Layout.fillWidth: true
            text: L10n.t("Make and answer calls on this computer")
            enabled: pane.hasHistory && pane.control.offered === true
            checked: pane.control.enabled === true
            // Switching on explains first, the first time on this computer.
            onToggled: {
                if (checked && intro.needed()) {
                    checked = Qt.binding(() => pane.control.enabled === true)
                    intro.open()
                } else {
                    EngineBridge.perform("calls.enable", String(checked))
                }
            }
        }

        // While call history is off, the note says why the switch is disabled.
        SettingsNote {
            visible: !pane.hasHistory
            text: L10n.t("Turn on call history to make and answer calls on this computer.")
        }

        RowLayout {
            Layout.fillWidth: true
            visible: pane.control.enabled === true && pane.hasHistory
            spacing: 8

            Text {
                Layout.fillWidth: true
                text: L10n.t("Bluetooth")
                color: Theme.text
                font.pixelSize: Theme.bodySize
            }

            Text {
                text: pane.statusText
                color: Theme.secondaryText
                font.pixelSize: Theme.bodySize
            }
        }

        CallNotice {
            Layout.fillWidth: true
            visible: pane.control.enabled === true && pane.hasHistory && !!pane.control.connectionNotice
            kind: pane.control.connectionNotice || ""
            connecting: pane.control.connecting === true
        }

        Rectangle {
            Layout.fillWidth: true
            implicitHeight: 1
            color: Theme.separator
        }

        RowLayout {
            Layout.fillWidth: true
            spacing: 8

            Icon {
                Layout.alignment: Qt.AlignTop
                name: "handset"
                size: 14
                color: Theme.secondaryText
            }

            SettingsNote {
                text: L10n.t("Calls stay on your phone. This computer places, answers and ends them over Bluetooth, and you talk on your phone or on a headset connected to it.")
            }
        }

        AbstractButton {
            id: emergency

            hoverEnabled: true
            padding: 0
            text: L10n.t("Contacting emergency services using Phone Link")
            Accessible.role: Accessible.Link
            Accessible.name: text
            onClicked: Qt.openUrlExternally(pane.emergencyInfo)

            HoverHandler {
                cursorShape: Qt.PointingHandCursor
            }

            background: Rectangle {
                radius: 3
                color: "transparent"
                border.color: emergency.visualFocus ? Theme.accent : "transparent"
            }

            contentItem: Text {
                text: emergency.text
                color: Theme.accent
                font.pixelSize: Theme.subheadSize
                font.underline: emergency.hovered
            }
        }
    }

    CallControlIntroDialog {
        id: intro
    }
}
