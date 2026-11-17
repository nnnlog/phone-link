import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import PhoneLink

// The Messages section's detail: the shown conversation, the new message, or what was sent to new
// recipients, each with its header, its messages (or the contacts matching "To:") and the composer,
// which keeps its place for every detail. Images dropped on it join the draft. While messages are off
// or not allowed it stands aside; the list shows what to do.
Rectangle {
    id: detailView

    property var detail: null
    readonly property string kind: detail !== null && detail.kind !== null ? detail.kind : ""
    readonly property bool gated: detail !== null && detail.gated
    readonly property var header: detail !== null ? detail.header : null
    readonly property var callControl: { EngineBridge.callsRevision; return JSON.parse(EngineBridge.query("callControl", "")) }
    readonly property var callsState: { EngineBridge.callsRevision; return JSON.parse(EngineBridge.query("callsState", "")) }
    // A one-person conversation's number can be called while call control is offered and call history
    // is on, which it needs.
    readonly property bool callOffered: header !== null && header.callNumber !== null && callControl !== null
        && callControl.offered === true && callsState !== null && callsState.access !== "consentRequired"
        && callsState.access !== "unsupported"
    readonly property bool canDial: callOffered && callControl.enabled === true && callControl.status === "ready"

    function reload() {
        detail = JSON.parse(EngineBridge.query("messages.detail", ""))
    }

    // Image bytes as base64, for image data dropped without a file.
    function base64(buffer) {
        const table = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"
        const bytes = new Uint8Array(buffer)
        const chunks = []
        let i = 0
        for (; i + 2 < bytes.length; i += 3) {
            const n = (bytes[i] << 16) | (bytes[i + 1] << 8) | bytes[i + 2]
            chunks.push(table[n >> 18 & 63] + table[n >> 12 & 63] + table[n >> 6 & 63] + table[n & 63])
        }
        if (i < bytes.length) {
            const n = (bytes[i] << 16) | ((i + 1 < bytes.length ? bytes[i + 1] : 0) << 8)
            chunks.push(table[n >> 18 & 63] + table[n >> 12 & 63]
                        + (i + 1 < bytes.length ? table[n >> 6 & 63] : "=") + "=")
        }
        return chunks.join("")
    }

    color: Theme.windowBackground
    Component.onCompleted: reload()
    onKindChanged: {
        if (kind === "newMessage")
            Qt.callLater(recipientHeader.focusField)
    }

    Connections {
        target: EngineBridge
        function onAreaEvent(area, name, payload) {
            if (area === "messages" && name === "detail")
                detailView.reload()
        }
    }

    EmptyState {
        anchors.centerIn: parent
        width: parent.width - 80
        visible: detailView.detail !== null && !detailView.gated && detailView.kind === ""
        title: L10n.t("No Conversation Selected")
    }

    ColumnLayout {
        anchors.fill: parent
        visible: !detailView.gated && detailView.kind !== ""
        spacing: 0

        // Who the conversation is with, and Call for a one-person conversation.
        RowLayout {
            Layout.fillWidth: true
            Layout.minimumHeight: 52
            Layout.leftMargin: 20
            Layout.rightMargin: 16
            visible: detailView.kind === "conversation" && detailView.header !== null
            spacing: 10

            MessagesAvatar {
                group: detailView.header !== null && detailView.header.group
                name: detailView.header !== null ? detailView.header.name || "" : ""
                photo: detailView.header !== null ? detailView.header.photo || "" : ""
                size: 28
            }

            ColumnLayout {
                Layout.fillWidth: true
                spacing: 0

                Text {
                    Layout.fillWidth: true
                    text: detailView.header !== null ? detailView.header.title : ""
                    color: Theme.text
                    font.pixelSize: Theme.bodySize + 1
                    font.weight: Font.Bold
                    wrapMode: Text.Wrap
                    maximumLineCount: 2
                    elide: Text.ElideRight
                }

                TextEdit {
                    Layout.fillWidth: true
                    visible: detailView.header !== null && detailView.header.subtitle !== null
                    text: detailView.header !== null && detailView.header.subtitle !== null ? detailView.header.subtitle : ""
                    color: Theme.secondaryText
                    font.pixelSize: Theme.subheadSize
                    textFormat: TextEdit.PlainText
                    readOnly: true
                    selectByMouse: true
                    selectionColor: Theme.accent
                    selectedTextColor: "white"
                }
            }

            IconButton {
                visible: detailView.callOffered
                enabled: detailView.canDial
                iconName: "phone"
                label: !detailView.callOffered ? ""
                     : detailView.callControl.enabled !== true ? L10n.t("Turn on calls on this computer")
                     : !detailView.canDial ? L10n.t("This computer isn't connected to your phone")
                     : L10n.f("Call %@ using your phone", [detailView.header.callNumber])
                // A disabled button shows no tooltip: say why on the hover anyway.
                HoverHandler {
                    id: callHover
                }
                ToolTip.visible: callHover.hovered && label.length > 0
                onClicked: EngineBridge.perform("calls.dial", detailView.header.callNumber)
            }
        }

        MessagesRecipientHeader {
            id: recipientHeader
            Layout.fillWidth: true
            visible: detailView.kind === "newMessage" || detailView.kind === "unsent"
            detail: detailView.detail
        }

        Rectangle {
            Layout.fillWidth: true
            implicitHeight: 1
            color: Theme.separator
        }

        Item {
            Layout.fillWidth: true
            Layout.fillHeight: true

            MessagesSuggestionList {
                anchors.fill: parent
                visible: suggestions.length > 0
                suggestions: detailView.detail !== null ? detailView.detail.suggestions : []
                onAdded: recipientHeader.focusField()
            }

            ColumnLayout {
                anchors.centerIn: parent
                width: parent.width - 80
                visible: detailView.detail !== null && detailView.detail.suggestions.length === 0
                         && detailView.detail.transcriptFailed
                spacing: 14

                EmptyState {
                    Layout.fillWidth: true
                    iconName: "alertCircle"
                    title: L10n.t("Messages Couldn't Be Read")
                    description: L10n.t("Phone Link couldn't read this conversation's messages.")
                }

                PillButton {
                    Layout.alignment: Qt.AlignHCenter
                    text: L10n.t("Try Again")
                    onClicked: EngineBridge.perform("messages.reloadTranscript", "")
                }
            }

            MessagesTranscript {
                anchors.fill: parent
                visible: detailView.detail !== null && detailView.detail.suggestions.length === 0
                         && !detailView.detail.transcriptFailed
                owner: detailView.detail !== null && detailView.detail.key !== null ? detailView.detail.key : ""
            }
        }

        MessagesComposer {
            Layout.fillWidth: true
            Layout.leftMargin: 14
            Layout.rightMargin: 14
            Layout.topMargin: 8
            Layout.bottomMargin: 12
            detail: detailView.detail
        }
    }

    DropArea {
        anchors.fill: parent
        enabled: !detailView.gated && detailView.kind !== ""
        onEntered: drag => {
            drag.accepted = drag.urls.some(url => url.toString().startsWith("file:"))
                || drag.formats.some(format => format.startsWith("image/"))
        }
        onDropped: drop => {
            const files = drop.urls.map(url => url.toString()).filter(url => url.startsWith("file:"))
            if (files.length > 0) {
                EngineBridge.perform("messages.addFiles", JSON.stringify(files))
                drop.acceptProposedAction()
                return
            }
            const format = drop.formats.find(f => f.startsWith("image/"))
            if (format !== undefined) {
                const data = drop.getDataAsArrayBuffer(format)
                EngineBridge.perform("messages.addImageData", JSON.stringify({ data: detailView.base64(data), type: format }))
                drop.acceptProposedAction()
            }
        }
    }
}
