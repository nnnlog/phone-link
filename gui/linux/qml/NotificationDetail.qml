import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import PhoneLink

// The Notifications section's detail column: the selected notification in full — its app and time, its
// text, the phone's buttons and where it opens — with Pin and Dismiss at the top and, when it takes a
// reply, the reply field at the bottom.
Rectangle {
    id: detail

    property var notification: null
    readonly property string key: notification ? notification.key : ""
    // Unsent replies by notification key, so a reply never goes to the notification opened next.
    property var drafts: ({})
    property string draftKey

    // Bumped on the notifications part's `changed` event, so the state below is read again.
    property int revision: 0
    readonly property var status: {
        revision
        EngineBridge.mirroringRevision
        return JSON.parse(EngineBridge.query("notifications.state", "")) || ({ content: "loading" })
    }
    readonly property bool opensInMessages: notification !== null && notification.openTarget === "messaging"
    // While the phone screen and its apps open on this computer, a notification the phone can launch
    // opens on the phone, in a mirroring window here. A message's notification opens in Messages
    // instead, and a call's has no open command.
    readonly property bool opensOnPhone: notification !== null && status.openOnPhone === true
                                         && notification.launchSupported && notification.openTarget === "notifications"
    readonly property bool hasButtons: notification !== null && (notification.actions.length > 0 || opensInMessages || opensOnPhone)

    color: Theme.windowBackground

    onKeyChanged: {
        if (draftKey.length > 0)
            drafts[draftKey] = reply.text
        reply.text = drafts[key] || ""
        draftKey = key
        scroller.contentY = 0
    }

    Connections {
        target: EngineBridge

        function onAreaEvent(area, name, payload) {
            if (area !== "notifications")
                return
            if (name === "changed") {
                detail.revision++
            } else if (name === "replyQueued") {
                const key = JSON.parse(payload).key
                delete detail.drafts[key]
                if (key === detail.key)
                    reply.text = ""
            } else if (name === "focusReply") {
                const key = JSON.parse(payload).key
                Qt.callLater(() => {
                    if (key === detail.key && reply.visible)
                        reply.forceActiveFocus()
                })
            }
        }
    }

    component Selectable: TextEdit {
        Layout.fillWidth: true
        color: Theme.text
        font.pixelSize: Theme.bodySize + 1
        wrapMode: TextEdit.Wrap
        textFormat: TextEdit.PlainText
        readOnly: true
        selectByMouse: true
        selectionColor: Theme.accent
        selectedTextColor: "white"
    }

    // The list column already says why there is no list.
    EmptyState {
        anchors.centerIn: parent
        width: parent.width - 80
        visible: detail.notification === null && detail.status.content === "list"
        iconName: "bell"
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
                readonly property bool pinned: detail.notification !== null && detail.notification.pinned
                iconName: pinned ? "pinSlash" : "pin"
                label: pinned ? L10n.t("Unpin") : L10n.t("Pin")
                onClicked: EngineBridge.setNotificationPinned(detail.key, !pinned)
            }

            IconButton {
                iconName: "close"
                label: L10n.t("Dismiss")
                onClicked: EngineBridge.dismissNotification(detail.key)
            }
        }

        Flickable {
            id: scroller

            Layout.fillWidth: true
            Layout.fillHeight: true
            contentHeight: body.implicitHeight + 40
            clip: true
            boundsBehavior: Flickable.StopAtBounds
            ScrollBar.vertical: ScrollBar {}

            ColumnLayout {
                id: body

                x: 26
                y: 12
                // Longer lines read poorly in a wide window.
                width: Math.min(scroller.width - 52, 560)
                spacing: 16

                // The app's icon and name over when the notification was posted.
                RowLayout {
                    spacing: 10
                    Accessible.role: Accessible.StaticText
                    Accessible.name: detail.notification ? detail.notification.app + ", " + posted.text : ""

                    AppGlyph {
                        name: detail.notification ? detail.notification.app : ""
                        packageName: detail.notification ? detail.notification["package"] : ""
                        size: 28
                    }

                    ColumnLayout {
                        spacing: 1

                        Text {
                            text: detail.notification ? detail.notification.app : ""
                            color: Theme.text
                            font.pixelSize: Theme.bodySize
                            font.weight: Font.DemiBold
                        }

                        Text {
                            id: posted
                            text: detail.notification ? Dates.detailStamp(detail.notification.timeMs) : ""
                            color: Theme.secondaryText
                            font.pixelSize: Theme.captionSize + 1
                        }
                    }
                }

                // The title and subtitle, then the body or the conversation.
                ColumnLayout {
                    Layout.fillWidth: true
                    spacing: 4

                    Selectable {
                        visible: text.length > 0
                        text: detail.notification ? detail.notification.title : ""
                        font.pixelSize: Theme.titleSize - 2
                        font.weight: Font.DemiBold
                    }

                    Selectable {
                        visible: text.length > 0
                        text: detail.notification ? detail.notification.subtitle : ""
                        color: Theme.secondaryText
                    }

                    Selectable {
                        Layout.topMargin: 4
                        visible: detail.notification !== null && detail.notification.conversation.length === 0
                                 && text.length > 0
                        text: detail.notification ? detail.notification.body : ""
                    }

                    // A messaging-style notification: each sender in bold beside their messages.
                    ColumnLayout {
                        Layout.fillWidth: true
                        Layout.topMargin: 4
                        visible: detail.notification !== null && detail.notification.conversation.length > 0
                        spacing: 6

                        Repeater {
                            model: detail.notification ? detail.notification.conversation : []

                            RowLayout {
                                id: run

                                required property var modelData

                                Layout.fillWidth: true
                                spacing: 6

                                Selectable {
                                    Layout.fillWidth: false
                                    Layout.maximumWidth: body.width * 0.4
                                    Layout.alignment: Qt.AlignTop
                                    visible: run.modelData.sender !== null
                                    text: run.modelData.sender || ""
                                    font.weight: Font.DemiBold
                                }

                                ColumnLayout {
                                    Layout.fillWidth: true
                                    Layout.alignment: Qt.AlignTop
                                    spacing: 2

                                    Repeater {
                                        model: run.modelData.messages

                                        Selectable {
                                            required property string modelData
                                            text: modelData
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                // The phone's own buttons, then where the notification opens.
                Flow {
                    Layout.fillWidth: true
                    visible: detail.hasButtons
                    spacing: 8

                    Repeater {
                        model: detail.notification ? detail.notification.actions : []

                        PillButton {
                            required property var modelData
                            text: modelData.name
                            onClicked: EngineBridge.pressNotificationAction(detail.key, modelData.index)
                        }
                    }

                    PillButton {
                        visible: detail.opensInMessages
                        text: L10n.t("Open in Messages")
                        onClicked: EngineBridge.showSection("messages")
                    }

                    PillButton {
                        visible: detail.opensOnPhone
                        text: L10n.t("Open on Phone")
                        ToolTip.visible: hovered
                        ToolTip.delay: 600
                        ToolTip.text: L10n.t("Open it in a window showing your phone")
                        onClicked: EngineBridge.perform("mirror.launchNotification", detail.key)
                    }
                }
            }
        }

        // Return or the send button sends; the text stays until the reply is queued.
        NotificationReplyField {
            id: reply
            Layout.fillWidth: true
            Layout.margins: 14
            visible: detail.notification !== null && detail.notification.replyIndex !== null
            placeholderText: detail.notification && detail.notification.replyName ? detail.notification.replyName : L10n.t("Reply")
            onSendRequested: text => EngineBridge.perform("notifications.reply", JSON.stringify({
                key: detail.key, index: detail.notification.replyIndex, text: text
            }))
        }
    }
}
