import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import PhoneLink

// The Calls section's detail column: the selected call's person — their picture, name and number over
// what can be done with the number (call it, message it, show the contact) — and every call with the
// number. Nothing while the list column shows a prompt in place of the calls.
Rectangle {
    id: detail

    // The calls with the selected call's number, newest first; the selected call alone for a private
    // number.
    property var calls: []
    // From the list column (areaEvent "calls" "listShown").
    property double selectedId: -1
    property string listContent: ""

    readonly property var call: calls.find(c => c.id === selectedId) || (calls.length > 0 ? calls[0] : null)
    readonly property var control: { EngineBridge.callsRevision; return JSON.parse(EngineBridge.query("callControl", "")) || {} }
    // The card's widest line, as the contact card's.
    readonly property int cardWidth: 560

    color: Theme.windowBackground

    Connections {
        target: EngineBridge

        function onAreaEvent(area, name, payload) {
            if (area !== "calls" || name !== "listShown")
                return
            const shown = JSON.parse(payload)
            detail.listContent = shown.content
            detail.selectedId = shown.selectedId
        }
    }

    CallsFormat {
        id: format
    }

    EmptyState {
        anchors.centerIn: parent
        width: parent.width - 80
        visible: detail.listContent === "list" && detail.call === null
        iconName: "phone"
        title: L10n.t("No Call Selected")
    }

    Flickable {
        id: flick

        anchors.fill: parent
        visible: detail.listContent === "list" && detail.call !== null
        contentHeight: card.implicitHeight + 48
        clip: true
        ScrollBar.vertical: ScrollBar {}

        ColumnLayout {
            id: card

            readonly property var call: detail.call || { id: -1, name: null, number: "", contactId: null, photo: null }
            readonly property bool hasNumber: call.number.length > 0

            x: (flick.width - width) / 2
            y: 24
            width: Math.min(detail.cardWidth, flick.width - 48)
            spacing: 20

            // The picture, the name, and the number under a contact's name.
            ColumnLayout {
                Layout.fillWidth: true
                spacing: 6

                Avatar {
                    Layout.alignment: Qt.AlignHCenter
                    Layout.bottomMargin: 4
                    name: card.call.name || ""
                    photo: card.call.photo || ""
                    size: 88
                }

                TextEdit {
                    Layout.fillWidth: true
                    horizontalAlignment: TextEdit.AlignHCenter
                    text: format.name(card.call)
                    color: Theme.text
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
                    visible: !!card.call.name && card.hasNumber
                    horizontalAlignment: TextEdit.AlignHCenter
                    text: card.call.number
                    color: Theme.secondaryText
                    font.pixelSize: Theme.bodySize + 1
                    readOnly: true
                    selectByMouse: true
                    selectionColor: Theme.accent
                    selectedTextColor: "white"
                }
            }

            RowLayout {
                Layout.alignment: Qt.AlignHCenter
                spacing: 12

                CallButton {
                    number: card.call.number
                    control: detail.control
                }

                CallPill {
                    text: L10n.t("Message")
                    iconName: "message"
                    available: card.hasNumber
                    help: card.hasNumber ? L10n.f("Send a message to %@", [card.call.number]) : L10n.t("A private number can't be messaged")
                    onClicked: EngineBridge.perform("messages.compose", JSON.stringify({ addresses: [card.call.number], text: "" }))
                }

                CallPill {
                    visible: card.call.contactId !== null && card.call.contactId !== undefined
                    text: L10n.t("Contact")
                    iconName: "person"
                    help: L10n.t("Show in Contacts")
                    onClicked: EngineBridge.perform("contacts.show", String(card.call.contactId))
                }
            }

            // Every call with the number.
            Rectangle {
                Layout.fillWidth: true
                implicitHeight: entries.implicitHeight
                radius: 10
                color: Theme.listBackground
                border.color: Theme.separator

                ColumnLayout {
                    id: entries

                    anchors.left: parent.left
                    anchors.right: parent.right
                    spacing: 0

                    Repeater {
                        model: detail.calls.length > 0 ? detail.calls : (detail.call ? [detail.call] : [])

                        Item {
                            id: entry

                            required property var modelData
                            required property int index
                            readonly property var style: format.style(modelData.kind)
                            readonly property bool selected: modelData.id === card.call.id

                            Layout.fillWidth: true
                            implicitHeight: entryRow.implicitHeight + 16

                            Accessible.role: Accessible.StaticText
                            Accessible.name: [format.fullTime(modelData.startMs), style.label, format.duration(modelData.durationSecs)]
                                             .filter(part => part.length > 0).join(", ")

                            Rectangle {
                                anchors.left: parent.left
                                anchors.right: parent.right
                                anchors.leftMargin: 14
                                anchors.rightMargin: 14
                                visible: entry.index > 0
                                height: 1
                                color: Theme.separator
                            }

                            RowLayout {
                                id: entryRow

                                anchors.left: parent.left
                                anchors.right: parent.right
                                anchors.verticalCenter: parent.verticalCenter
                                anchors.leftMargin: 14
                                anchors.rightMargin: 14
                                spacing: 8

                                Icon {
                                    Layout.preferredWidth: 20
                                    name: entry.style.icon.length > 0 ? entry.style.icon : "phone"
                                    opacity: entry.style.icon.length > 0 ? 1 : 0
                                    size: 15
                                    color: entry.style.missed ? Theme.destructive : Theme.secondaryText
                                }

                                ColumnLayout {
                                    Layout.fillWidth: true
                                    spacing: 2

                                    Text {
                                        Layout.fillWidth: true
                                        text: format.fullTime(entry.modelData.startMs)
                                        color: Theme.text
                                        font.pixelSize: Theme.bodySize
                                        font.weight: entry.selected ? Font.DemiBold : Font.Normal
                                        elide: Text.ElideRight
                                    }

                                    Text {
                                        Layout.fillWidth: true
                                        visible: text.length > 0
                                        text: entry.style.label
                                        color: entry.style.missed ? Theme.destructive : Theme.secondaryText
                                        font.pixelSize: Theme.captionSize
                                        elide: Text.ElideRight
                                    }
                                }

                                Text {
                                    visible: text.length > 0
                                    text: format.duration(entry.modelData.durationSecs)
                                    color: Theme.secondaryText
                                    font.pixelSize: Theme.bodySize
                                    font.features: { "tnum": 1 }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
