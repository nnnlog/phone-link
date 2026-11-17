import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import PhoneLink

// The running transfer in the sidebar: its file, how much is done, a 4 px progress bar under the name,
// and Stop for a send. Shown while a send or a receive runs.
Rectangle {
    id: card

    readonly property var transfers: { EngineBridge.transfersRevision; return JSON.parse(EngineBridge.query("transfers", "")) || {} }
    readonly property var transfer: transfers.card || null
    readonly property bool sending: transfers.sending === true

    visible: transfer !== null
    implicitHeight: content.implicitHeight + 16
    radius: Theme.moduleRadius
    color: Theme.sidebarModule

    Accessible.role: Accessible.Grouping
    Accessible.name: transfer ? [transfer.name, transfer.amount || ""].join(" ") : ""

    RowLayout {
        id: content
        anchors.fill: parent
        anchors.leftMargin: Theme.content - Theme.edge
        anchors.rightMargin: 6
        anchors.topMargin: 8
        anchors.bottomMargin: 8
        spacing: 8

        Rectangle {
            Layout.preferredWidth: 26
            Layout.preferredHeight: 26
            Layout.alignment: Qt.AlignVCenter
            radius: 13
            color: Theme.accent
            Accessible.ignored: true

            Icon {
                anchors.centerIn: parent
                name: card.sending ? "send" : "arrowDown"
                size: 18
                color: "white"
            }
        }

        ColumnLayout {
            Layout.fillWidth: true
            spacing: 5

            RowLayout {
                Layout.fillWidth: true
                spacing: 4

                Text {
                    Layout.fillWidth: true
                    text: card.transfer ? card.transfer.name : ""
                    color: Theme.text
                    font.pixelSize: Theme.subheadSize
                    font.weight: Font.DemiBold
                    elide: Text.ElideMiddle
                }

                Text {
                    visible: text.length > 0
                    text: card.transfer && card.transfer.amount ? card.transfer.amount : ""
                    color: Theme.secondaryText
                    font.pixelSize: Theme.captionSize
                }
            }

            // While the size is unknown, a sliver marks the transfer as started.
            Rectangle {
                Layout.fillWidth: true
                implicitHeight: 4
                radius: 2
                color: Theme.controlBackground

                Rectangle {
                    width: Math.max(4, parent.width * (card.transfer && card.transfer.fraction !== null ? card.transfer.fraction : 0))
                    height: parent.height
                    radius: 2
                    color: Theme.accent
                }
            }
        }

        IconButton {
            Layout.alignment: Qt.AlignVCenter
            visible: card.sending
            implicitWidth: 24
            implicitHeight: 24
            iconName: "close"
            label: L10n.t("Stop Sending")
            onClicked: EngineBridge.perform("files.cancel", "")
        }
    }
}
