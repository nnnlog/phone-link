import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import PhoneLink

// The Messages section's list: the new message being written, what was sent to new recipients that
// no conversation holds yet, the conversations newest first, and the hidden ones in a section of their
// own — or the state that stands in for them: messages off on this computer, not allowed on the phone,
// the first sync, or none at all.
Rectangle {
    id: column

    property var list: null
    readonly property string status: list ? list.status : "empty"
    readonly property bool gated: list !== null && list.gated
    property bool showsHidden: false
    // Hide and Show Conversation, undone with Ctrl+Z and done again with Ctrl+Shift+Z.
    property var undoStack: []
    property var redoStack: []

    function reload() {
        list = JSON.parse(EngineBridge.query("messages.list", ""))
    }

    function rows(list) {
        if (list === null)
            return []
        const result = []
        if (list.newMessage)
            result.push(Object.assign({ kind: "new" }, list.newMessage))
        list.unsent.forEach(group => result.push(Object.assign({ kind: "unsent" }, group)))
        list.conversations.forEach(c => result.push(Object.assign({ kind: "conversation" }, c)))
        if (list.hidden.length > 0) {
            const expanded = showsHidden || list.searching
            result.push({ kind: "header", key: "header:hidden", expanded: expanded })
            if (expanded)
                list.hidden.forEach(c => result.push(Object.assign({ kind: "conversation" }, c)))
        }
        return result
    }

    function select(item) {
        EngineBridge.perform("messages.select", JSON.stringify(item))
    }

    // Deleting a new message that holds something asks first.
    function discardNewMessage(hasContent) {
        if (hasContent)
            discardConfirmation.open()
        else
            EngineBridge.perform("messages.discardNewMessage", "")
    }

    function newMessage() {
        if (!gated)
            EngineBridge.perform("messages.newMessage", "")
    }

    function setHidden(threadId, hidden, undoing) {
        EngineBridge.perform("messages.setHidden", JSON.stringify({ threadId: threadId, on: hidden }))
        const action = { threadId: threadId, hidden: hidden }
        if (undoing === "undo")
            redoStack = redoStack.concat([action])
        else
            undoStack = undoStack.concat([action])
        if (undoing === undefined)
            redoStack = []
    }

    function undo() {
        if (undoStack.length === 0)
            return
        const last = undoStack[undoStack.length - 1]
        undoStack = undoStack.slice(0, -1)
        setHidden(last.threadId, !last.hidden, "undo")
    }

    function redo() {
        if (redoStack.length === 0)
            return
        const last = redoStack[redoStack.length - 1]
        redoStack = redoStack.slice(0, -1)
        setHidden(last.threadId, !last.hidden, "redo")
    }

    color: Theme.listBackground
    Component.onCompleted: reload()
    onListChanged: rowModel.sync(rows(list), row => row.key)
    onShowsHiddenChanged: rowModel.sync(rows(list), row => row.key)

    Connections {
        target: EngineBridge
        function onAreaEvent(area, name, payload) {
            if (area === "messages" && name === "list")
                column.reload()
        }
    }

    Shortcut {
        sequence: "Ctrl+N"
        enabled: column.visible && !column.gated
        onActivated: column.newMessage()
    }

    Shortcut {
        sequence: StandardKey.Undo
        enabled: column.visible && column.undoStack.length > 0
        onActivated: column.undo()
    }

    Shortcut {
        sequences: [StandardKey.Redo]
        enabled: column.visible && column.redoStack.length > 0
        onActivated: column.redo()
    }

    KeyedListModel {
        id: rowModel
    }

    ColumnLayout {
        anchors.fill: parent
        spacing: 0

        RowLayout {
            Layout.fillWidth: true
            Layout.preferredHeight: 52
            Layout.leftMargin: 20
            Layout.rightMargin: 10
            spacing: 8

            Text {
                Layout.fillWidth: true
                text: L10n.t("Messages")
                color: Theme.text
                font.pixelSize: Theme.headlineSize
                font.weight: Font.Bold
                elide: Text.ElideRight
            }

            IconButton {
                iconName: "compose"
                label: L10n.t("New Message")
                enabled: !column.gated
                onClicked: column.newMessage()
            }
        }

        // The search matches names, addresses and the latest message's preview, not every message.
        CapsuleField {
            id: search
            Layout.fillWidth: true
            Layout.leftMargin: 12
            Layout.rightMargin: 12
            Layout.bottomMargin: 8
            visible: column.status === "conversations"
            placeholderText: L10n.t("Search")
            submittable: false
            Accessible.name: L10n.t("Search")
            onTextChanged: EngineBridge.perform("messages.search", text)
            Keys.onEscapePressed: event => {
                if (text.length > 0) {
                    text = ""
                    event.accepted = true
                } else {
                    event.accepted = false
                }
            }
        }

        Item {
            Layout.fillWidth: true
            Layout.fillHeight: true

            ListView {
                id: listView
                anchors.fill: parent
                visible: column.status === "conversations"
                clip: true
                model: rowModel
                leftMargin: 10
                rightMargin: 10
                bottomMargin: 10
                ScrollBar.vertical: ScrollBar {}

                delegate: Loader {
                    id: entry
                    required property string json
                    readonly property var modelData: JSON.parse(json)
                    width: ListView.view.width - 20
                    sourceComponent: modelData.kind === "header" ? hiddenHeader
                                   : modelData.kind === "new" ? newMessageRow
                                   : modelData.kind === "unsent" ? unsentRow : conversationRow

                    Component {
                        id: hiddenHeader

                        AbstractButton {
                            id: header
                            implicitHeight: 34
                            hoverEnabled: true
                            Accessible.role: Accessible.Button
                            Accessible.name: L10n.t("Hidden")
                            onClicked: column.showsHidden = !entry.modelData.expanded

                            contentItem: RowLayout {
                                spacing: 4

                                Icon {
                                    Layout.leftMargin: 4
                                    name: "chevronRight"
                                    size: 12
                                    color: Theme.secondaryText
                                    rotation: entry.modelData.expanded ? 90 : 0
                                }

                                Text {
                                    Layout.fillWidth: true
                                    text: L10n.t("Hidden")
                                    color: Theme.secondaryText
                                    font.pixelSize: Theme.subheadSize
                                    font.weight: Font.DemiBold
                                }
                            }

                            background: Rectangle {
                                radius: Theme.rowRadius
                                color: header.hovered ? Theme.hover : "transparent"
                            }
                        }
                    }

                    Component {
                        id: newMessageRow

                        ConversationRow {
                            title: L10n.t("New Message")
                            preview: entry.modelData.preview
                            selected: column.list !== null && column.list.selection === entry.modelData.key
                            onClicked: column.select({ kind: "newMessage" })
                            onMenuRequested: (x, y) => newMessageMenu.popup(x, y)

                            leading: Rectangle {
                                width: 40
                                height: 40
                                radius: 20
                                color: Theme.controlBackground

                                Icon {
                                    anchors.centerIn: parent
                                    name: "compose"
                                    size: 18
                                    color: Theme.secondaryText
                                }
                            }

                            MessagesMenu {
                                id: newMessageMenu

                                MenuItem {
                                    text: L10n.t("Delete")
                                    onTriggered: column.discardNewMessage(entry.modelData.hasContent)
                                }
                            }
                        }
                    }

                    Component {
                        id: unsentRow

                        ConversationRow {
                            title: entry.modelData.title
                            preview: entry.modelData.preview
                            time: entry.modelData.latestMs !== null ? Dates.listStamp(entry.modelData.latestMs) : ""
                            selected: column.list !== null && column.list.selection === entry.modelData.key
                            onClicked: column.select({ kind: "unsent", addresses: entry.modelData.addresses })

                            leading: MessagesAvatar {
                                group: entry.modelData.group
                                name: entry.modelData.name || ""
                                photo: entry.modelData.photo || ""
                                size: 40
                            }
                        }
                    }

                    Component {
                        id: conversationRow

                        ConversationRow {
                            title: entry.modelData.title
                            preview: entry.modelData.preview
                            time: Dates.listStamp(entry.modelData.latestMs)
                            unread: entry.modelData.unread
                            muted: entry.modelData.muted
                            selected: column.list !== null && column.list.selection === entry.modelData.key
                            onClicked: column.select({ kind: "conversation", threadId: entry.modelData.threadId })
                            onMenuRequested: (x, y) => conversationMenu.popup(x, y)

                            leading: MessagesAvatar {
                                group: entry.modelData.group
                                name: entry.modelData.name || ""
                                photo: entry.modelData.photo || ""
                                size: 40
                            }

                            MessagesMenu {
                                id: conversationMenu

                                MenuItem {
                                    text: entry.modelData.muted ? L10n.t("Show Alerts") : L10n.t("Hide Alerts")
                                    onTriggered: EngineBridge.perform("messages.setMuted", JSON.stringify(
                                        { threadId: entry.modelData.threadId, on: !entry.modelData.muted }))
                                }

                                MenuSeparator {}

                                MenuItem {
                                    text: entry.modelData.hidden ? L10n.t("Show Conversation") : L10n.t("Hide Conversation")
                                    onTriggered: column.setHidden(entry.modelData.threadId, !entry.modelData.hidden)
                                }
                            }
                        }
                    }
                }

                EmptyState {
                    anchors.centerIn: parent
                    width: parent.width - 40
                    visible: column.list !== null && column.list.searching && rowModel.count === 0
                    iconName: "search"
                    title: L10n.f("No Results for “%@”", [search.text.trim()])
                    description: L10n.t("Check the spelling or try a new search.")
                }
            }

            // What stands in for the list.
            ColumnLayout {
                anchors.centerIn: parent
                width: parent.width - 40
                visible: column.status !== "conversations"
                spacing: 14

                EmptyState {
                    Layout.fillWidth: true
                    visible: column.status !== "syncing"
                    iconName: "message"
                    title: column.status === "turnedOff" ? L10n.t("Messages Are Off on This Computer")
                         : column.status === "permissionNeeded" ? L10n.t("Allow Messages on Your Phone")
                         : L10n.t("No Messages")
                    description: column.status === "turnedOff"
                                 ? L10n.t("Turn on messages to read and send your phone's texts on this computer.")
                                 : column.status === "permissionNeeded"
                                   ? L10n.f("Messages from %@ appear here after you allow access on the phone.",
                                            [EngineBridge.phoneName.length > 0 ? EngineBridge.phoneName : L10n.t("your phone")])
                                   : L10n.t("Text messages from your phone appear here.")
                }

                PillButton {
                    Layout.alignment: Qt.AlignHCenter
                    visible: column.status === "turnedOff"
                    prominent: true
                    text: L10n.t("Turn On")
                    onClicked: EngineBridge.perform("messages.turnOn", "")
                }

                PillButton {
                    Layout.alignment: Qt.AlignHCenter
                    visible: column.status === "permissionNeeded"
                    prominent: true
                    enabled: column.list !== null && !column.list.requestingPermission
                    text: L10n.t("Send Request to Phone")
                    onClicked: EngineBridge.perform("messages.requestPermission", "")
                }

                BusyIndicator {
                    Layout.alignment: Qt.AlignHCenter
                    visible: column.status === "syncing"
                    running: visible
                }

                Text {
                    Layout.fillWidth: true
                    visible: column.status === "syncing"
                    text: L10n.t("Syncing messages...")
                    color: Theme.secondaryText
                    font.pixelSize: Theme.bodySize
                    horizontalAlignment: Text.AlignHCenter
                    wrapMode: Text.Wrap
                }
            }
        }
    }

    MessageDialog {
        id: discardConfirmation
        heading: L10n.t("Delete This Message?")
        message: L10n.t("The message you're writing and its recipients will be deleted.")
        acceptText: L10n.t("Delete")
        destructive: true
        onAccepted: EngineBridge.perform("messages.discardNewMessage", "")
    }
}
