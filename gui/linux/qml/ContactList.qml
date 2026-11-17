import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import PhoneLink

// The Contacts section's list column: the phone's contacts under their index letters, filtered by the
// search once typing pauses — or the state that stands in for them: contacts off on this computer, not
// allowed on the phone, the first sync, or none at all. Nothing is sent to the phone when it shows; the
// phone is asked only when the user acts on the prompt.
//
// It tells the detail column what it shows (areaEvent "contacts" "listShown": {content}), and selects
// the contact a call's *Contact* asks for (areaEvent "contacts" "show": {id}).
Rectangle {
    id: column

    property var contacts: []
    property int selectedId: -1
    // The search the list follows: the field's text once typing paused.
    property string query

    readonly property var contactsState: { EngineBridge.contactsRevision; return JSON.parse(EngineBridge.query("contactsState", "")) }
    // What the column shows: "nothing", "turnedOff", "needsPhoneAccess", "syncing", "empty" or "list".
    readonly property string content: {
        if (contactsState === null)
            return "nothing"
        switch (contactsState.access) {
        case "consentRequired": return "turnedOff"
        case "permissionRequired":
        case "permissionDenied": return "needsPhoneAccess"
        case "granted": return contacts.length > 0 ? "list" : contactsState.syncing ? "syncing" : "empty"
        default: return "nothing"
        }
    }
    // The phone's name; a phone without one is "your phone", and "Your phone" where it starts a sentence.
    readonly property string phoneName: EngineBridge.phoneName.length > 0 ? EngineBridge.phoneName : L10n.t("your phone")
    readonly property string phoneNameAtStart: EngineBridge.phoneName.length > 0 ? EngineBridge.phoneName : L10n.t("Your phone")
    readonly property var rows: contactsIndex.rows(contacts, query)

    function tellDetail() {
        EngineBridge.areaEvent("contacts", "listShown", JSON.stringify({ content: content }))
    }

    // Brings the selected contact's row into view.
    function revealSelected() {
        for (let row = 0; row < rowModel.count; row++) {
            if (rowModel.get(row).key === "contact:" + selectedId) {
                list.positionViewAtIndex(row, ListView.Contain)
                return
            }
        }
    }

    color: Theme.listBackground
    onRowsChanged: rowModel.sync(rows, row => row.key)
    onContentChanged: tellDetail()
    // A contact that is gone is no longer selected.
    onContactsChanged: {
        if (selectedId >= 0 && !contacts.some(contact => contact.id === selectedId))
            selectedId = -1
    }
    Component.onCompleted: {
        rowModel.sync(rows, row => row.key)
        // After the detail column exists.
        Qt.callLater(tellDetail)
    }

    Connections {
        target: EngineBridge

        // A call's *Contact*: that contact, with the search cleared so the list shows it.
        function onAreaEvent(area, name, payload) {
            if (area !== "contacts" || name !== "show")
                return
            search.text = ""
            column.query = ""
            column.selectedId = JSON.parse(payload).id
            Qt.callLater(column.revealSelected)
        }
    }

    ContactsIndex {
        id: contactsIndex
    }

    KeyedListModel {
        id: rowModel
    }

    Timer {
        id: searchDelay
        interval: 300
        onTriggered: column.query = search.text.trim()
    }

    Shortcut {
        sequence: StandardKey.Find
        enabled: column.visible && column.content === "list"
        onActivated: search.forceActiveFocus()
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
                text: L10n.t("Contacts")
                color: Theme.text
                font.pixelSize: Theme.headlineSize
                font.weight: Font.Bold
            }

            BusyIndicator {
                visible: column.content === "list" && column.contactsState !== null && column.contactsState.syncing === true
                running: visible
                padding: 1
                implicitWidth: 20
                implicitHeight: 20
                Accessible.name: L10n.t("Syncing contacts...")
                ToolTip.visible: syncingHover.hovered
                ToolTip.delay: 600
                ToolTip.text: L10n.t("Syncing contacts...")

                HoverHandler {
                    id: syncingHover
                }
            }
        }

        CapsuleField {
            id: search

            Layout.fillWidth: true
            Layout.leftMargin: 12
            Layout.rightMargin: 12
            Layout.bottomMargin: 8
            visible: column.content === "list"
            placeholderText: L10n.t("Search")
            submittable: false
            Accessible.name: L10n.t("Search")
            Keys.onEscapePressed: text = ""
            // Clearing the field shows the whole list at once.
            onTextChanged: {
                if (text.trim().length === 0) {
                    searchDelay.stop()
                    column.query = ""
                } else {
                    searchDelay.restart()
                }
            }
        }

        ListView {
            id: list

            Layout.fillWidth: true
            Layout.fillHeight: true
            visible: column.content === "list"
            clip: true
            model: rowModel
            leftMargin: 10
            rightMargin: 10
            bottomMargin: 10
            ScrollBar.vertical: ScrollBar {}

            delegate: Loader {
                required property string json
                readonly property var modelData: JSON.parse(json)
                width: ListView.view.width - 20
                sourceComponent: modelData.header !== undefined ? headerRow : contactRow

                Component {
                    id: headerRow

                    Item {
                        implicitHeight: 32

                        Text {
                            anchors.left: parent.left
                            anchors.leftMargin: 6
                            anchors.bottom: parent.bottom
                            anchors.bottomMargin: 6
                            text: modelData.header
                            color: Theme.secondaryText
                            font.pixelSize: Theme.subheadSize
                            font.weight: Font.DemiBold
                            Accessible.role: Accessible.Heading
                            Accessible.name: text
                        }

                        Rectangle {
                            anchors.left: parent.left
                            anchors.right: parent.right
                            anchors.bottom: parent.bottom
                            height: 1
                            color: Theme.separator
                        }
                    }
                }

                Component {
                    id: contactRow

                    AbstractButton {
                        id: row

                        readonly property var contact: modelData.contact

                        implicitHeight: 44
                        hoverEnabled: true
                        Accessible.role: Accessible.Button
                        Accessible.name: nameText.text
                        onClicked: column.selectedId = contact.id

                        background: Rectangle {
                            radius: Theme.rowRadius
                            color: column.selectedId === row.contact.id ? Theme.selection
                                 : row.hovered ? Theme.hover : "transparent"
                            border.color: row.visualFocus ? Theme.accent : "transparent"
                        }

                        contentItem: RowLayout {
                            spacing: 10

                            Avatar {
                                Layout.leftMargin: 6
                                name: row.contact.name
                                photo: row.contact.photo || ""
                                size: 32
                            }

                            Text {
                                id: nameText

                                Layout.fillWidth: true
                                text: row.contact.name.length > 0 ? row.contact.name : L10n.t("No Name")
                                color: row.contact.name.length > 0 ? Theme.text : Theme.secondaryText
                                font.pixelSize: Theme.bodySize + 1
                                elide: Text.ElideRight
                            }
                        }
                    }
                }
            }

            EmptyState {
                anchors.centerIn: parent
                width: parent.width - 40
                visible: column.rows.length === 0 && column.query.length > 0
                iconName: "search"
                title: L10n.f("No Results for “%@”", [column.query])
                description: L10n.t("Check the spelling or try a new search.")
            }
        }

        // What stands in for the list.
        Item {
            Layout.fillWidth: true
            Layout.fillHeight: true
            visible: column.content !== "list"

            ColumnLayout {
                anchors.centerIn: parent
                width: parent.width - 40
                visible: column.content !== "nothing" && column.content !== "syncing"
                spacing: 14

                EmptyState {
                    Layout.fillWidth: true
                    iconName: "person"
                    title: {
                        switch (column.content) {
                        case "turnedOff": return L10n.t("Contacts Are Off on This Computer")
                        case "needsPhoneAccess": return L10n.t("Allow Contacts on Your Phone")
                        default: return L10n.t("No Contacts")
                        }
                    }
                    description: {
                        switch (column.content) {
                        case "turnedOff":
                            return L10n.t("Turn on contacts to see your phone's contacts on this computer.")
                        case "needsPhoneAccess":
                            if (column.contactsState.timedOut === true)
                                return L10n.t("Your phone didn't respond. Unlock it and try again.")
                            if (column.contactsState.access === "permissionDenied")
                                return L10n.f("%@ stopped sharing its contacts. Allow access on the phone to see them again.", [column.phoneNameAtStart])
                            return L10n.f("Contacts from %@ appear here after you allow access on the phone.", [column.phoneName])
                        default:
                            return L10n.t("Contacts from your phone appear here.")
                        }
                    }
                }

                PillButton {
                    Layout.alignment: Qt.AlignHCenter
                    visible: column.content === "turnedOff" || (column.content === "needsPhoneAccess" && column.contactsState.requesting !== true)
                    prominent: true
                    text: column.content === "turnedOff" ? L10n.t("Turn On") : L10n.t("Send Request to Phone")
                    onClicked: EngineBridge.perform(column.content === "turnedOff" ? "contacts.turnOn" : "contacts.requestPermission", "")
                }

                RowLayout {
                    Layout.alignment: Qt.AlignHCenter
                    visible: column.content === "needsPhoneAccess" && column.contactsState.requesting === true
                    spacing: 8

                    BusyIndicator {
                        running: parent.visible
                        padding: 1
                        implicitWidth: 20
                        implicitHeight: 20
                    }

                    Text {
                        text: L10n.t("Waiting for your phone...")
                        color: Theme.secondaryText
                        font.pixelSize: Theme.subheadSize
                    }
                }
            }

            ColumnLayout {
                anchors.centerIn: parent
                visible: column.content === "syncing"
                spacing: 10

                BusyIndicator {
                    Layout.alignment: Qt.AlignHCenter
                    running: parent.visible
                }

                Text {
                    Layout.alignment: Qt.AlignHCenter
                    text: L10n.t("Syncing contacts...")
                    color: Theme.secondaryText
                    font.pixelSize: Theme.bodySize
                }
            }
        }
    }
}
