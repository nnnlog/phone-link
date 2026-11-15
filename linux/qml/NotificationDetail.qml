import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import PhoneLink

Rectangle {
    id: detail

    property var notification: null

    function escaped(text) {
        return text.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;")
    }

    color: Theme.windowBackground

    EmptyState {
        anchors.centerIn: parent
        width: parent.width - 80
        visible: detail.notification === null
        title: L10n.t("No Notification Selected")
    }

    ColumnLayout {
        anchors.fill: parent
        visible: detail.notification !== null
        spacing: 0

        RowLayout {
            Layout.fillWidth: true
            Layout.preferredHeight: 52
            Layout.rightMargin: 10
            spacing: 6

            Item {
                Layout.fillWidth: true
            }

            IconButton {
                iconName: "pin"
                active: detail.notification !== null && detail.notification.pinned
                onClicked: EngineBridge.setNotificationPinned(detail.notification.key, !detail.notification.pinned)
            }

            IconButton {
                iconName: "close"
                onClicked: EngineBridge.dismissNotification(detail.notification.key)
            }
        }

        Flickable {
            Layout.fillWidth: true
            Layout.fillHeight: true
            contentHeight: body.implicitHeight + 40
            clip: true
            ScrollBar.vertical: ScrollBar {}

            ColumnLayout {
                id: body
                x: 26
                y: 20
                width: parent.width - 52
                spacing: 10

                RowLayout {
                    spacing: 10

                    AppGlyph {
                        name: detail.notification ? detail.notification.app : ""
                        packageName: detail.notification ? detail.notification["package"] : ""
                        size: 28
                    }

                    ColumnLayout {
                        spacing: 0

                        Text {
                            text: detail.notification ? detail.notification.app : ""
                            color: Theme.text
                            font.pixelSize: Theme.bodySize
                            font.weight: Font.DemiBold
                        }

                        Text {
                            text: detail.notification ? Dates.detailStamp(detail.notification.timeMs) : ""
                            color: Theme.secondaryText
                            font.pixelSize: Theme.captionSize
                        }
                    }
                }

                Text {
                    Layout.fillWidth: true
                    Layout.topMargin: 6
                    visible: text.length > 0
                    text: detail.notification ? detail.notification.title : ""
                    color: Theme.text
                    font.pixelSize: Theme.titleSize - 2
                    font.weight: Font.Bold
                    wrapMode: Text.Wrap
                }

                Text {
                    Layout.fillWidth: true
                    visible: detail.notification !== null && detail.notification.subtitle.length > 0
                    text: detail.notification ? detail.notification.subtitle : ""
                    color: Theme.secondaryText
                    font.pixelSize: Theme.bodySize + 1
                    wrapMode: Text.Wrap
                }

                Text {
                    Layout.fillWidth: true
                    visible: detail.notification !== null && detail.notification.conversation.length === 0
                    text: detail.notification ? detail.notification.body : ""
                    color: Theme.text
                    font.pixelSize: Theme.bodySize + 1
                    wrapMode: Text.Wrap
                    textFormat: Text.PlainText
                }

                Repeater {
                    model: detail.notification ? detail.notification.conversation : []

                    Text {
                        required property var modelData
                        Layout.fillWidth: true
                        text: (modelData.sender.length > 0 ? "<b>" + detail.escaped(modelData.sender) + "</b>&nbsp;&nbsp;" : "")
                              + detail.escaped(modelData.text)
                        color: Theme.text
                        font.pixelSize: Theme.bodySize + 1
                        wrapMode: Text.Wrap
                        textFormat: Text.StyledText
                    }
                }

                Flow {
                    Layout.fillWidth: true
                    Layout.topMargin: 6
                    spacing: 8
                    visible: detail.notification !== null && detail.notification.actions.length > 0

                    Repeater {
                        model: detail.notification ? detail.notification.actions : []

                        PillButton {
                            required property var modelData
                            text: modelData.name
                            onClicked: EngineBridge.pressNotificationAction(detail.notification.key, modelData.index)
                        }
                    }
                }
            }
        }

        CapsuleField {
            Layout.fillWidth: true
            Layout.margins: 14
            visible: detail.notification !== null && detail.notification.replyIndex !== null
            placeholderText: detail.notification && detail.notification.replyName ? detail.notification.replyName : L10n.t("Reply")
            onSubmitted: text => EngineBridge.replyToNotification(detail.notification.key, detail.notification.replyIndex, text)
        }
    }
}
