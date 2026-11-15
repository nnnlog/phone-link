import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import PhoneLink

Rectangle {
    id: column

    property var conversations: []

    color: Theme.listBackground

    ColumnLayout {
        anchors.fill: parent
        spacing: 0

        Text {
            Layout.fillWidth: true
            Layout.preferredHeight: 52
            Layout.leftMargin: 20
            verticalAlignment: Text.AlignVCenter
            text: L10n.t("Messages")
            color: Theme.text
            font.pixelSize: Theme.headlineSize
            font.weight: Font.Bold
        }

        ListView {
            Layout.fillWidth: true
            Layout.fillHeight: true
            clip: true
            model: column.conversations
            leftMargin: 10
            rightMargin: 10
            bottomMargin: 10
            ScrollBar.vertical: ScrollBar {}

            delegate: AbstractButton {
                id: row
                required property var modelData
                readonly property bool selected: EngineBridge.openThreadId === modelData.threadId
                width: ListView.view.width - 20
                implicitHeight: 64
                hoverEnabled: true
                onClicked: EngineBridge.openConversation(modelData.threadId)

                background: Rectangle {
                    radius: Theme.rowRadius
                    color: row.selected ? Theme.selection : row.hovered ? Theme.hover : "transparent"
                }

                contentItem: RowLayout {
                    spacing: 8

                    Rectangle {
                        Layout.preferredWidth: 7
                        Layout.preferredHeight: 7
                        Layout.leftMargin: 2
                        radius: 3.5
                        color: row.modelData.unread > 0 ? Theme.accent : "transparent"
                    }

                    Monogram {
                        name: row.modelData.isGroup ? "" : row.modelData.name
                        size: 40
                    }

                    ColumnLayout {
                        Layout.fillWidth: true
                        Layout.rightMargin: 6
                        spacing: 1

                        RowLayout {
                            Layout.fillWidth: true

                            Text {
                                Layout.fillWidth: true
                                text: row.modelData.name
                                color: Theme.text
                                font.pixelSize: Theme.bodySize + 1
                                font.weight: Font.Bold
                                elide: Text.ElideRight
                            }

                            Text {
                                text: Dates.listStamp(row.modelData.latestMs)
                                color: Theme.secondaryText
                                font.pixelSize: Theme.captionSize
                            }
                        }

                        Text {
                            Layout.fillWidth: true
                            text: row.modelData.preview
                            color: Theme.secondaryText
                            font.pixelSize: Theme.subheadSize
                            wrapMode: Text.Wrap
                            maximumLineCount: 2
                            elide: Text.ElideRight
                        }
                    }
                }
            }

            EmptyState {
                anchors.centerIn: parent
                width: parent.width - 40
                visible: column.conversations.length === 0
                iconName: "message"
                title: L10n.t("No Conversations")
            }
        }
    }
}
