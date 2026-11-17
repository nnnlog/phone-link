import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Dialogs
import QtQuick.Layouts
import PhoneLink

// The shown detail's messages, oldest first, opened at the newest: a day and time before a new stretch,
// group events as centered text, and each message as a bubble — received on the left, sent on the
// right — with its attachments, its text and how its send went (Try Again when it failed).
ListView {
    id: transcript

    // The shown detail (its composer key): another one opens at its newest message.
    property string owner
    readonly property var rows: JSON.parse(EngineBridge.messagesJson)
    property string shownOwner
    // At the end, it follows the bubbles as they lay out and grow, until the user scrolls away.
    property bool pinned: true

    // Updates the transcript in place. It moves to the end for another detail or a new message, and
    // stays at the end while the user has not scrolled away; otherwise it stays where it was.
    function syncRows() {
        const lastKey = rows.length > 0 ? rows[rows.length - 1].key : ""
        let arrived = lastKey.length > 0
        for (let i = 0; i < rowModel.count && arrived; i++)
            arrived = rowModel.get(i).key !== lastKey
        rowModel.sync(rows, row => row.key)
        if (shownOwner !== owner || arrived)
            pinned = true
        shownOwner = owner
        if (pinned)
            Qt.callLater(showEnd)
    }

    // positionViewAtEnd() lays out the last bubbles but leaves out the bottom margin.
    function showEnd() {
        positionViewAtEnd()
        contentY = Math.max(originY - topMargin, originY + contentHeight + bottomMargin - height)
    }

    clip: true
    model: rowModel
    spacing: 6
    topMargin: 16
    bottomMargin: 16
    leftMargin: 20
    rightMargin: 20
    ScrollBar.vertical: ScrollBar {}

    onRowsChanged: syncRows()
    onOwnerChanged: syncRows()
    Component.onCompleted: syncRows()
    onContentHeightChanged: if (pinned) Qt.callLater(showEnd)
    onHeightChanged: if (pinned) Qt.callLater(showEnd)
    onMovementStarted: pinned = false
    onMovementEnded: pinned = atYEnd

    KeyedListModel {
        id: rowModel
    }

    FileDialog {
        id: saveDialog
        property string key
        fileMode: FileDialog.SaveFile
        onAccepted: EngineBridge.perform("messages.savePart", JSON.stringify({ key: key, url: selectedFile.toString() }))
    }

    delegate: ColumnLayout {
        id: entry
        required property string json
        readonly property var row: JSON.parse(json)
        readonly property bool outgoing: !row.incoming
        width: ListView.view.width - 40
        spacing: 4

        Text {
            Layout.alignment: Qt.AlignHCenter
            Layout.topMargin: 6
            visible: entry.row.separator
            text: Dates.detailStamp(entry.row.timestampMs)
            color: Theme.secondaryText
            font.pixelSize: Theme.captionSize
        }

        Text {
            Layout.fillWidth: true
            visible: entry.row.event !== null
            text: entry.row.event || ""
            color: Theme.secondaryText
            font.pixelSize: Theme.captionSize
            horizontalAlignment: Text.AlignHCenter
            wrapMode: Text.Wrap
        }

        RowLayout {
            Layout.fillWidth: true
            visible: entry.row.event === null
            spacing: 6
            // Right to left for a sent message: Try Again, then its bubbles, then the space.
            layoutDirection: entry.outgoing ? Qt.RightToLeft : Qt.LeftToRight

            AbstractButton {
                id: retry
                Layout.alignment: Qt.AlignVCenter
                implicitWidth: 24
                implicitHeight: 24
                visible: entry.row.sendId !== null
                enabled: !entry.row.retrying
                hoverEnabled: true
                Accessible.role: Accessible.Button
                Accessible.name: L10n.t("Try Again")
                ToolTip.visible: hovered
                ToolTip.delay: 600
                ToolTip.text: L10n.t("Try Again")
                onClicked: EngineBridge.perform("messages.retry", String(entry.row.sendId))

                contentItem: Item {
                    Icon {
                        anchors.centerIn: parent
                        name: "alertCircle"
                        size: 20
                        color: Theme.destructive
                        opacity: retry.enabled ? (retry.hovered ? 0.8 : 1) : 0.4
                    }
                }
            }

            ColumnLayout {
                id: stack
                Layout.maximumWidth: Math.min(entry.width * 0.7, 520)
                spacing: 3

                Text {
                    Layout.alignment: entry.outgoing ? Qt.AlignRight : Qt.AlignLeft
                    Layout.leftMargin: 12
                    Layout.rightMargin: 12
                    visible: entry.row.sender !== null
                    text: entry.row.sender || ""
                    color: Theme.secondaryText
                    font.pixelSize: Theme.captionSize
                }

                Repeater {
                    model: entry.row.parts

                    MessagesPart {
                        required property var modelData
                        Layout.alignment: entry.outgoing ? Qt.AlignRight : Qt.AlignLeft
                        Layout.maximumWidth: stack.Layout.maximumWidth
                        part: modelData
                        onSaveRequested: (key, fileName) => {
                            saveDialog.key = key
                            saveDialog.currentFile = encodeURIComponent(fileName)
                            saveDialog.open()
                        }
                    }
                }

                Rectangle {
                    Layout.alignment: entry.outgoing ? Qt.AlignRight : Qt.AlignLeft
                    Layout.maximumWidth: stack.Layout.maximumWidth
                    visible: entry.row.body.length > 0 || entry.row.subject.length > 0
                    implicitWidth: Math.max(subjectText.visible ? subjectText.implicitWidth : 0, bodyText.implicitWidth) + 24
                    implicitHeight: bubbleText.implicitHeight + 12
                    radius: 15
                    color: entry.outgoing ? Theme.outgoingBubble : Theme.incomingBubble
                    opacity: entry.row.sendState === "sending" ? 0.6 : 1

                    ColumnLayout {
                        id: bubbleText
                        anchors.fill: parent
                        anchors.leftMargin: 12
                        anchors.rightMargin: 12
                        anchors.topMargin: 6
                        anchors.bottomMargin: 6
                        spacing: 2

                        TextEdit {
                            id: subjectText
                            Layout.fillWidth: true
                            visible: entry.row.subject.length > 0
                            text: entry.row.subject
                            color: entry.outgoing ? "white" : Theme.text
                            font.pixelSize: Theme.bodySize + 1
                            font.weight: Font.Bold
                            wrapMode: TextEdit.Wrap
                            textFormat: TextEdit.PlainText
                            readOnly: true
                            selectByMouse: true
                            selectionColor: entry.outgoing ? "white" : Theme.accent
                            selectedTextColor: entry.outgoing ? Theme.accent : "white"
                        }

                        TextEdit {
                            id: bodyText
                            Layout.fillWidth: true
                            visible: entry.row.body.length > 0
                            text: entry.row.body
                            color: entry.outgoing ? "white" : Theme.text
                            font.pixelSize: Theme.bodySize + 1
                            wrapMode: TextEdit.Wrap
                            textFormat: TextEdit.PlainText
                            readOnly: true
                            selectByMouse: true
                            selectionColor: entry.outgoing ? "white" : Theme.accent
                            selectedTextColor: entry.outgoing ? Theme.accent : "white"
                        }
                    }
                }

                Text {
                    Layout.alignment: entry.outgoing ? Qt.AlignRight : Qt.AlignLeft
                    Layout.maximumWidth: stack.Layout.maximumWidth
                    visible: entry.row.status !== null
                    text: entry.row.status || ""
                    color: entry.row.statusError ? Theme.destructive : Theme.secondaryText
                    font.pixelSize: Theme.captionSize
                    wrapMode: Text.Wrap
                }
            }

            Item {
                Layout.fillWidth: true
            }
        }
    }
}
