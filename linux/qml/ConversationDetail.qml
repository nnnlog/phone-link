import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import PhoneLink

Rectangle {
    id: detail

    property var conversation: null
    readonly property var messages: JSON.parse(EngineBridge.messagesJson)

    function showsStamp(index) {
        return index === 0 || messages[index].timestampMs - messages[index - 1].timestampMs > 3600000
    }

    color: Theme.windowBackground

    EmptyState {
        anchors.centerIn: parent
        width: parent.width - 80
        visible: detail.conversation === null
        title: L10n.t("No Conversation Selected")
    }

    ColumnLayout {
        anchors.fill: parent
        visible: detail.conversation !== null
        spacing: 0

        RowLayout {
            Layout.fillWidth: true
            Layout.preferredHeight: 52
            Layout.leftMargin: 20
            Layout.rightMargin: 20
            spacing: 10

            Monogram {
                name: detail.conversation && !detail.conversation.isGroup ? detail.conversation.name : ""
                size: 28
            }

            ColumnLayout {
                Layout.fillWidth: true
                spacing: 0

                Text {
                    Layout.fillWidth: true
                    text: detail.conversation ? detail.conversation.name : ""
                    color: Theme.text
                    font.pixelSize: Theme.bodySize + 1
                    font.weight: Font.Bold
                    elide: Text.ElideRight
                }

                Text {
                    visible: text.length > 0 && text !== (detail.conversation ? detail.conversation.name : "")
                    text: detail.conversation ? detail.conversation.address : ""
                    color: Theme.secondaryText
                    font.pixelSize: Theme.captionSize
                }
            }
        }

        Rectangle {
            Layout.fillWidth: true
            implicitHeight: 1
            color: Theme.separator
        }

        ListView {
            id: transcript
            Layout.fillWidth: true
            Layout.fillHeight: true
            clip: true
            model: detail.messages
            spacing: 6
            topMargin: 16
            bottomMargin: 16
            leftMargin: 20
            rightMargin: 20
            ScrollBar.vertical: ScrollBar {}
            onCountChanged: Qt.callLater(positionViewAtEnd)

            delegate: ColumnLayout {
                id: entry
                required property var modelData
                required property int index
                width: ListView.view.width - 40
                spacing: 6

                Text {
                    Layout.alignment: Qt.AlignHCenter
                    Layout.topMargin: 6
                    visible: detail.showsStamp(entry.index)
                    text: Dates.detailStamp(entry.modelData.timestampMs)
                    color: Theme.secondaryText
                    font.pixelSize: Theme.captionSize
                }

                Rectangle {
                    Layout.alignment: entry.modelData.incoming ? Qt.AlignLeft : Qt.AlignRight
                    Layout.maximumWidth: entry.width * 0.7
                    implicitWidth: bubbleText.implicitWidth + 24
                    implicitHeight: bubbleText.implicitHeight + 12
                    radius: 15
                    color: entry.modelData.incoming ? Theme.incomingBubble : Theme.outgoingBubble
                    opacity: entry.modelData.sendState === "sending" ? 0.6 : 1

                    Text {
                        id: bubbleText
                        anchors.fill: parent
                        anchors.leftMargin: 12
                        anchors.rightMargin: 12
                        anchors.topMargin: 6
                        anchors.bottomMargin: 6
                        text: entry.modelData.body.length > 0 ? entry.modelData.body
                            : entry.modelData.attachments > 0 ? L10n.t("Attachment") : ""
                        color: entry.modelData.incoming ? Theme.text : "white"
                        font.pixelSize: Theme.bodySize + 1
                        wrapMode: Text.Wrap
                        textFormat: Text.PlainText
                    }
                }

                Text {
                    Layout.alignment: Qt.AlignRight
                    visible: entry.modelData.sendState === "failed"
                    text: L10n.t("Not Delivered")
                    color: "#ff453a"
                    font.pixelSize: Theme.captionSize
                }
            }
        }

        CapsuleField {
            Layout.fillWidth: true
            Layout.margins: 14
            visible: detail.conversation !== null && detail.conversation.canReply
            placeholderText: L10n.t("Text Message")
            onSubmitted: text => EngineBridge.sendMessage(text)
        }
    }
}
