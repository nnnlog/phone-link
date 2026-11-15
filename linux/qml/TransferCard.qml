import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import PhoneLink

Rectangle {
    id: card

    readonly property var transfers: { EngineBridge.transfersRevision; return JSON.parse(EngineBridge.query("transfers", "")) || {} }
    readonly property var transfer: transfers.send || transfers.receive || null
    readonly property bool sending: !!transfers.send

    visible: transfer !== null
    implicitHeight: content.implicitHeight + 20
    radius: Theme.moduleRadius
    color: Theme.sidebarModule

    ColumnLayout {
        id: content
        anchors.fill: parent
        anchors.margins: 10
        spacing: 6

        RowLayout {
            Layout.fillWidth: true

            Icon {
                name: "upload"
                size: 16
                color: Theme.accent
            }

            Text {
                Layout.fillWidth: true
                text: card.transfer
                      ? (card.transfer.files.length === 1 ? card.transfer.files[0]
                                                           : L10n.f("%lld files", [String(card.transfer.files.length)]))
                      : ""
                color: Theme.text
                font.pixelSize: Theme.bodySize
                font.weight: Font.DemiBold
                elide: Text.ElideMiddle
            }

            IconButton {
                visible: card.sending
                implicitWidth: 22
                implicitHeight: 22
                iconName: "close"
                onClicked: EngineBridge.perform("files.cancel", "")
            }
        }

        ProgressBar {
            id: progress
            Layout.fillWidth: true
            from: 0
            to: card.transfer && card.transfer.total > 0 ? card.transfer.total : 1
            value: card.transfer ? card.transfer.done : 0
            indeterminate: !card.transfer || card.transfer.total === 0

            background: Rectangle {
                implicitHeight: 4
                radius: 2
                color: Theme.controlBackground
            }

            contentItem: Item {
                Rectangle {
                    width: progress.visualPosition * parent.width
                    height: parent.height
                    radius: 2
                    color: Theme.accent
                }
            }
        }

        Text {
            text: L10n.t(card.sending ? "Sending to your phone..." : "Receiving from your phone...")
            color: Theme.secondaryText
            font.pixelSize: Theme.captionSize
        }
    }
}
