import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import PhoneLink

Rectangle {
    id: banner

    readonly property var control: { EngineBridge.callsRevision; return JSON.parse(EngineBridge.query("callControl", "")) || {} }
    readonly property var call: control.active && control.active.length > 0 ? control.active[0] : null

    function statusText(status) {
        switch (status) {
        case "incoming": return L10n.t("Incoming Call")
        case "dialing": return L10n.t("Calling...")
        case "held": return L10n.t("On Hold")
        default: return L10n.t("Connected")
        }
    }

    visible: call !== null
    width: 360
    implicitHeight: content.implicitHeight + 24
    radius: 16
    color: Theme.dark ? "#2c2c2e" : "#ffffff"
    border.color: Theme.separator

    RowLayout {
        id: content
        anchors.fill: parent
        anchors.margins: 12
        spacing: 12

        Avatar {
            name: banner.call ? banner.call.name || "" : ""
            photo: banner.call ? banner.call.photo || "" : ""
            size: 44
        }

        ColumnLayout {
            Layout.fillWidth: true
            spacing: 2

            Text {
                Layout.fillWidth: true
                text: banner.call ? banner.call.name || banner.call.number : ""
                color: Theme.text
                font.pixelSize: Theme.bodySize + 2
                font.weight: Font.Bold
                elide: Text.ElideRight
            }

            Text {
                text: banner.call ? banner.statusText(banner.call.status) : ""
                color: Theme.secondaryText
                font.pixelSize: Theme.subheadSize
            }
        }

        RoundButton {
            visible: banner.call !== null && banner.call.status === "incoming"
            implicitWidth: 40
            implicitHeight: 40
            onClicked: EngineBridge.perform("calls.answer", String(banner.call.id))
            background: Rectangle { radius: 20; color: "#30d158" }
            contentItem: Item {
                Icon { anchors.centerIn: parent; name: "phone"; filled: true; size: 18; color: "white" }
            }
        }

        RoundButton {
            implicitWidth: 40
            implicitHeight: 40
            onClicked: EngineBridge.perform(banner.call.status === "incoming" ? "calls.decline" : "calls.end", String(banner.call.id))
            background: Rectangle { radius: 20; color: "#ff453a" }
            contentItem: Item {
                Icon { anchors.centerIn: parent; name: "phone"; filled: true; size: 18; color: "white"; rotation: 135 }
            }
        }
    }
}
