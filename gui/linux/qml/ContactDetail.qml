import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import PhoneLink

// The Contacts section's detail column: the selected contact's card — their picture, name, nickname and
// work over their numbers (each with *Call* and *Message*), email addresses (write an email), postal
// addresses (show on a map), links (open in the browser), dates and notes; every field has Copy in
// its menu. Nothing while the list column shows a prompt in place of the contacts.
Rectangle {
    id: detail

    property int contactId: -1
    // From the list column (areaEvent "contacts" "listShown").
    property string listContent: ""

    readonly property var card: {
        EngineBridge.contactsRevision
        return contactId < 0 ? null : JSON.parse(EngineBridge.query("contact", String(contactId)))
    }
    readonly property var control: { EngineBridge.callsRevision; return JSON.parse(EngineBridge.query("callControl", "")) || {} }
    // *Call* shows on numbers while calls can be placed from this computer at all.
    readonly property bool callable: control.offered === true && control.pcConsent === true
    // The card's widest line: longer lines read poorly in a wide window.
    readonly property int cardWidth: 560

    function subtitle(card) {
        const parts = [card.jobTitle, card.company === card.name ? "" : card.company].filter(part => part.length > 0)
        return parts.join(" · ")
    }

    function openMenu(value) {
        copyMenu.value = value
        copyMenu.popup()
    }

    color: Theme.windowBackground

    Connections {
        target: EngineBridge

        function onAreaEvent(area, name, payload) {
            if (area === "contacts" && name === "listShown")
                detail.listContent = JSON.parse(payload).content
        }
    }

    ContactsIndex {
        id: contactsIndex
    }

    CallClipboard {
        id: clipboard
    }

    CallMenu {
        id: copyMenu

        property string value

        CallMenuItem {
            text: L10n.t("Copy")
            onTriggered: clipboard.copy(copyMenu.value)
        }
    }

    // A field's label over its value, an action at the trailing edge, and Copy in its menu (right
    // click, or the menu key); a line above it when it follows another field of its group.
    component FieldRow: Item {
        id: field

        property string label
        property string value
        property bool divided: false
        default property alias content: valueSlot.data
        property alias trailing: trailingSlot.data

        Layout.fillWidth: true
        implicitHeight: fieldRow.implicitHeight + 16

        TapHandler {
            acceptedButtons: Qt.RightButton
            onTapped: detail.openMenu(field.value)
        }

        Rectangle {
            anchors.left: parent.left
            anchors.right: parent.right
            visible: field.divided
            height: 1
            color: Theme.separator
        }

        RowLayout {
            id: fieldRow

            anchors.left: parent.left
            anchors.right: parent.right
            anchors.verticalCenter: parent.verticalCenter
            spacing: 8

            ColumnLayout {
                Layout.fillWidth: true
                spacing: 2

                Text {
                    Layout.fillWidth: true
                    text: field.label
                    color: Theme.secondaryText
                    font.pixelSize: Theme.captionSize
                    elide: Text.ElideRight
                }

                Item {
                    id: valueSlot

                    Layout.fillWidth: true
                    implicitHeight: childrenRect.height
                }
            }

            RowLayout {
                id: trailingSlot

                spacing: 6
            }
        }
    }

    // A field's value as text the user can select.
    component FieldText: TextEdit {
        width: parent ? parent.width : 0
        color: Theme.text
        font.pixelSize: Theme.bodySize + 1
        wrapMode: TextEdit.Wrap
        readOnly: true
        selectByMouse: true
        selectionColor: Theme.accent
        selectedTextColor: "white"
        Keys.onMenuPressed: detail.openMenu(text)
    }

    // A field's value that opens something: an email to it, a map of it, the link.
    component FieldLink: AbstractButton {
        id: link

        property string help
        property string target

        width: parent ? Math.min(parent.width, implicitWidth) : implicitWidth
        hoverEnabled: true
        padding: 0
        Accessible.role: Accessible.Link
        Accessible.name: text
        Accessible.description: help
        ToolTip.visible: hovered && help.length > 0
        ToolTip.delay: 600
        ToolTip.text: help
        Keys.onMenuPressed: detail.openMenu(text)
        onClicked: Qt.openUrlExternally(target)

        HoverHandler {
            cursorShape: Qt.PointingHandCursor
        }

        background: Rectangle {
            radius: 3
            color: "transparent"
            border.color: link.visualFocus ? Theme.accent : "transparent"
        }

        contentItem: Text {
            text: link.text
            color: Theme.accent
            font.pixelSize: Theme.bodySize + 1
            font.underline: link.hovered
            wrapMode: Text.Wrap
        }
    }

    // The fields of one kind on the card's rounded background.
    component FieldGroup: Rectangle {
        default property alias rows: groupColumn.data

        Layout.fillWidth: true
        implicitHeight: groupColumn.implicitHeight
        radius: 10
        color: Theme.listBackground
        border.color: Theme.separator

        ColumnLayout {
            id: groupColumn

            anchors.left: parent.left
            anchors.right: parent.right
            anchors.leftMargin: 14
            anchors.rightMargin: 14
            spacing: 0
        }
    }

    EmptyState {
        anchors.centerIn: parent
        width: parent.width - 80
        visible: detail.listContent === "list" && detail.card === null && detail.contactId < 0
        iconName: "person"
        title: L10n.t("No Contact Selected")
    }

    Flickable {
        id: flick

        anchors.fill: parent
        visible: detail.listContent === "list" && detail.card !== null
        contentHeight: cardColumn.implicitHeight + 48
        clip: true
        ScrollBar.vertical: ScrollBar {}

        ColumnLayout {
            id: cardColumn

            readonly property var card: detail.card || { id: -1, name: "", nickname: "", company: "", jobTitle: "", notes: "",
                                                         photo: null, phones: [], emails: [], addresses: [], urls: [], dates: [] }

            x: (flick.width - width) / 2
            y: 24
            width: Math.min(detail.cardWidth, flick.width - 48)
            spacing: 20

            // The picture, the name, and under it the nickname and the work.
            ColumnLayout {
                Layout.fillWidth: true
                spacing: 6

                Avatar {
                    Layout.alignment: Qt.AlignHCenter
                    Layout.bottomMargin: 4
                    name: cardColumn.card.name
                    photo: cardColumn.card.photo || ""
                    size: 88
                }

                TextEdit {
                    Layout.fillWidth: true
                    horizontalAlignment: TextEdit.AlignHCenter
                    text: cardColumn.card.name.length > 0 ? cardColumn.card.name : L10n.t("No Name")
                    color: cardColumn.card.name.length > 0 ? Theme.text : Theme.secondaryText
                    font.pixelSize: Theme.titleSize
                    font.weight: Font.DemiBold
                    wrapMode: TextEdit.Wrap
                    readOnly: true
                    selectByMouse: true
                    selectionColor: Theme.accent
                    selectedTextColor: "white"
                }

                TextEdit {
                    Layout.fillWidth: true
                    visible: cardColumn.card.nickname.length > 0 && cardColumn.card.nickname !== cardColumn.card.name
                    horizontalAlignment: TextEdit.AlignHCenter
                    text: L10n.f("“%@”", [cardColumn.card.nickname])
                    color: Theme.secondaryText
                    font.pixelSize: Theme.bodySize + 1
                    wrapMode: TextEdit.Wrap
                    readOnly: true
                    selectByMouse: true
                    selectionColor: Theme.accent
                    selectedTextColor: "white"
                }

                TextEdit {
                    Layout.fillWidth: true
                    visible: text.length > 0
                    horizontalAlignment: TextEdit.AlignHCenter
                    text: detail.subtitle(cardColumn.card)
                    color: Theme.secondaryText
                    font.pixelSize: Theme.bodySize + 1
                    wrapMode: TextEdit.Wrap
                    readOnly: true
                    selectByMouse: true
                    selectionColor: Theme.accent
                    selectedTextColor: "white"
                }
            }

            FieldGroup {
                visible: cardColumn.card.phones.length > 0

                Repeater {
                    model: cardColumn.card.phones

                    FieldRow {
                        id: phoneRow

                        required property var modelData
                        required property int index

                        label: modelData.label
                        value: modelData.value
                        divided: index > 0

                        FieldText {
                            text: phoneRow.modelData.value
                        }

                        trailing: [
                            CallButton {
                                visible: detail.callable
                                iconOnly: true
                                number: phoneRow.modelData.value
                                control: detail.control
                            },
                            CallPill {
                                iconOnly: true
                                iconName: "message"
                                text: L10n.t("Message")
                                help: L10n.f("Send a message to %@", [phoneRow.modelData.value])
                                onClicked: EngineBridge.perform("messages.compose",
                                                                JSON.stringify({ addresses: [phoneRow.modelData.number], text: "" }))
                            }
                        ]
                    }
                }
            }

            FieldGroup {
                visible: cardColumn.card.emails.length > 0

                Repeater {
                    model: cardColumn.card.emails

                    FieldRow {
                        id: emailRow

                        required property var modelData
                        required property int index

                        label: modelData.label
                        value: modelData.value
                        divided: index > 0

                        FieldLink {
                            text: emailRow.modelData.value
                            help: L10n.f("Write an email to %@", [emailRow.modelData.value])
                            target: "mailto:" + contactsIndex.escaped(emailRow.modelData.value)
                        }
                    }
                }
            }

            FieldGroup {
                visible: cardColumn.card.addresses.length > 0

                Repeater {
                    model: cardColumn.card.addresses

                    FieldRow {
                        id: addressRow

                        required property var modelData
                        required property int index

                        label: modelData.label
                        value: modelData.value
                        divided: index > 0

                        FieldLink {
                            text: addressRow.modelData.value
                            help: L10n.t("Show in OpenStreetMap")
                            target: "https://www.openstreetmap.org/search?query=" + contactsIndex.escaped(addressRow.modelData.value)
                        }
                    }
                }
            }

            FieldGroup {
                visible: cardColumn.card.urls.length > 0

                Repeater {
                    model: cardColumn.card.urls

                    FieldRow {
                        id: urlRow

                        required property var modelData
                        required property int index

                        label: modelData.label
                        value: modelData.value
                        divided: index > 0

                        FieldLink {
                            text: urlRow.modelData.value
                            help: L10n.t("Open in your browser")
                            target: contactsIndex.link(urlRow.modelData.value)
                        }
                    }
                }
            }

            FieldGroup {
                visible: cardColumn.card.dates.length > 0

                Repeater {
                    model: cardColumn.card.dates

                    FieldRow {
                        id: dateRow

                        required property var modelData
                        required property int index

                        label: modelData.label
                        value: modelData.value
                        divided: index > 0

                        FieldText {
                            text: dateRow.modelData.value
                        }
                    }
                }
            }

            FieldGroup {
                visible: cardColumn.card.notes.length > 0

                FieldRow {
                    label: L10n.t("notes")
                    value: cardColumn.card.notes

                    FieldText {
                        text: cardColumn.card.notes
                    }
                }
            }
        }
    }
}
