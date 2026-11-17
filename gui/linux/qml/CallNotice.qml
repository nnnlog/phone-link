import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import PhoneLink

// A call-control notice in a strip: what the Bluetooth connection to the phone needs from the user, or
// why the last dial failed; its title and text, and its action — beside the text where there is room,
// else under it. While a connect runs, the connection's action waits.
Rectangle {
    id: strip

    // The connection's "bluetoothOff", "notPaired", "notConnected" or "connectionFailed", or a dial's
    // "noNetwork", "airplaneMode" or "failed".
    property string kind
    // A connect or pairing runs.
    property bool connecting: false

    readonly property bool dial: ["noNetwork", "airplaneMode", "failed"].includes(kind)
    // Title, text, action, and the action's perform name.
    readonly property var texts: {
        switch (kind) {
        case "bluetoothOff":
            return [L10n.t("Bluetooth Is Off on This Computer"),
                    L10n.t("Turn on Bluetooth to make and answer calls on this computer."),
                    L10n.t("Open Bluetooth Settings"), "calls.openBluetoothSettings"]
        case "notPaired":
            return [L10n.t("Phone Not Paired with This Computer"),
                    L10n.t("Keep your phone nearby with Bluetooth on, then pair it with this computer."),
                    L10n.t("Begin Pairing"), "calls.pair"]
        case "notConnected":
            return [L10n.t("Phone Not Connected"), L10n.t("Keep your phone nearby with Bluetooth on, then connect again."),
                    L10n.t("Connect"), "calls.connect"]
        case "connectionFailed":
            return [L10n.t("Couldn't Connect to Your Phone"),
                    L10n.t("Turn Bluetooth off and back on, on both your phone and this computer, then try again."),
                    L10n.t("Try Again"), "calls.connect"]
        case "noNetwork":
            return [L10n.t("Call Not Placed"),
                    L10n.t("Your phone isn't connected to a mobile network. Check its SIM card and signal, or place the call on your phone."),
                    L10n.t("OK"), "calls.dismissNotice"]
        case "airplaneMode":
            return [L10n.t("Call Not Placed"),
                    L10n.t("Your phone is in Airplane Mode. Turn off Airplane Mode on your phone, then try again."),
                    L10n.t("OK"), "calls.dismissNotice"]
        case "failed":
            return [L10n.t("Call Not Placed"),
                    L10n.t("Turn Bluetooth off and back on, on both your phone and this computer, then try again. If the call still isn't placed, place it on your phone."),
                    L10n.t("OK"), "calls.dismissNotice"]
        default:
            return ["", "", "", ""]
        }
    }
    readonly property bool waiting: connecting && !dial && kind !== "bluetoothOff"
    readonly property bool wide: width >= 480

    implicitHeight: layout.implicitHeight + 20
    radius: Theme.rowRadius
    color: Theme.controlBackground

    Accessible.role: Accessible.AlertMessage
    Accessible.name: texts[0] + ". " + texts[1]

    GridLayout {
        id: layout

        anchors.fill: parent
        anchors.margins: 10
        columns: strip.wide ? 3 : 2
        columnSpacing: 10
        rowSpacing: 8

        Icon {
            Layout.alignment: Qt.AlignTop
            Layout.preferredWidth: 20
            name: strip.dial ? "phoneDown" : "antennaSlash"
            size: 18
            color: Theme.secondaryText
        }

        ColumnLayout {
            Layout.fillWidth: true
            spacing: 2

            Text {
                Layout.fillWidth: true
                text: strip.texts[0]
                color: Theme.text
                font.pixelSize: Theme.bodySize
                font.weight: Font.DemiBold
                wrapMode: Text.Wrap
            }

            Text {
                Layout.fillWidth: true
                text: strip.texts[1]
                color: Theme.secondaryText
                font.pixelSize: Theme.subheadSize
                wrapMode: Text.Wrap
            }
        }

        // Under the icon when the action goes under the text.
        Item {
            visible: !strip.wide
            implicitWidth: 1
            implicitHeight: 1
        }

        Item {
            Layout.alignment: strip.wide ? Qt.AlignVCenter : Qt.AlignLeft
            implicitWidth: strip.waiting ? busy.implicitWidth : action.implicitWidth
            implicitHeight: action.implicitHeight

            PillButton {
                id: action
                visible: !strip.waiting
                text: strip.texts[2]
                onClicked: EngineBridge.perform(strip.texts[3], "")
            }

            BusyIndicator {
                id: busy
                anchors.verticalCenter: parent.verticalCenter
                visible: strip.waiting
                running: visible
                padding: 1
                implicitWidth: 20
                implicitHeight: 20
            }
        }
    }
}
