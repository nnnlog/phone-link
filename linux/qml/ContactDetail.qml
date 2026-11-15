import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import PhoneLink

Rectangle {
    id: detail

    property int contactId: -1
    readonly property var card: {
        EngineBridge.contactsRevision
        return contactId < 0 ? null : JSON.parse(EngineBridge.query("contact", String(contactId)))
    }
    readonly property var sections: card ? [
        { title: L10n.t("Phone"), items: card.phones },
        { title: L10n.t("Email"), items: card.emails },
        { title: L10n.t("Address"), items: card.addresses },
        { title: L10n.t("Website"), items: card.urls },
        { title: L10n.t("Date"), items: card.dates }
    ].filter(section => section.items.length > 0) : []

    color: Theme.windowBackground

    EmptyState {
        anchors.centerIn: parent
        width: parent.width - 80
        visible: detail.card === null
        title: L10n.t("No Contact Selected")
    }

    Flickable {
        anchors.fill: parent
        visible: detail.card !== null
        contentHeight: body.implicitHeight + 52
        clip: true
        ScrollBar.vertical: ScrollBar {}

        ColumnLayout {
            id: body
            x: 26
            y: 26
            width: parent.width - 52
            spacing: 10

            Avatar {
                Layout.alignment: Qt.AlignHCenter
                Layout.topMargin: 10
                name: detail.card ? detail.card.name : ""
                photo: detail.card ? detail.card.photo || "" : ""
                size: 80
            }

            Text {
                Layout.alignment: Qt.AlignHCenter
                text: detail.card ? detail.card.name : ""
                color: Theme.text
                font.pixelSize: Theme.titleSize
                font.weight: Font.Bold
            }

            Text {
                Layout.alignment: Qt.AlignHCenter
                visible: text.length > 0
                text: detail.card ? [detail.card.jobTitle, detail.card.company].filter(t => t.length > 0).join(", ") : ""
                color: Theme.secondaryText
                font.pixelSize: Theme.bodySize
            }

            Repeater {
                model: detail.sections

                Rectangle {
                    id: block
                    required property var modelData
                    Layout.fillWidth: true
                    Layout.topMargin: 6
                    implicitHeight: fields.implicitHeight + 16
                    radius: Theme.moduleRadius
                    color: Theme.listBackground
                    border.color: Theme.separator

                    ColumnLayout {
                        id: fields
                        anchors.fill: parent
                        anchors.margins: 8
                        spacing: 8

                        Repeater {
                            model: block.modelData.items

                            ColumnLayout {
                                required property var modelData
                                Layout.fillWidth: true
                                Layout.leftMargin: 6
                                spacing: 1

                                Text {
                                    text: modelData.label.length > 0 ? modelData.label : block.modelData.title
                                    color: Theme.secondaryText
                                    font.pixelSize: Theme.captionSize
                                }

                                TextEdit {
                                    Layout.fillWidth: true
                                    text: modelData.value
                                    color: Theme.text
                                    font.pixelSize: Theme.bodySize + 1
                                    readOnly: true
                                    selectByMouse: true
                                    wrapMode: Text.Wrap
                                }
                            }
                        }
                    }
                }
            }

            Text {
                Layout.fillWidth: true
                Layout.topMargin: 6
                visible: detail.card !== null && detail.card.notes.length > 0
                text: detail.card ? detail.card.notes : ""
                color: Theme.text
                font.pixelSize: Theme.bodySize
                wrapMode: Text.Wrap
            }
        }
    }
}
