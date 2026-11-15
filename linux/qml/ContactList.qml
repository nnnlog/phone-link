import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import PhoneLink

Rectangle {
    id: column

    property var contacts: []
    property int selectedId: -1

    readonly property var shown: {
        const query = search.text.trim().toLowerCase()
        if (query.length === 0)
            return contacts
        return contacts.filter(c => c.name.toLowerCase().includes(query) || c.search.toLowerCase().includes(query))
    }

    color: Theme.listBackground

    ColumnLayout {
        anchors.fill: parent
        spacing: 0

        Text {
            Layout.fillWidth: true
            Layout.preferredHeight: 52
            Layout.leftMargin: 20
            verticalAlignment: Text.AlignVCenter
            text: L10n.t("Contacts")
            color: Theme.text
            font.pixelSize: Theme.headlineSize
            font.weight: Font.Bold
        }

        CapsuleField {
            id: search
            Layout.fillWidth: true
            Layout.leftMargin: 12
            Layout.rightMargin: 12
            Layout.bottomMargin: 8
            placeholderText: L10n.t("Search")
            submittable: false
        }

        ListView {
            Layout.fillWidth: true
            Layout.fillHeight: true
            clip: true
            model: column.shown
            leftMargin: 10
            rightMargin: 10
            bottomMargin: 10
            ScrollBar.vertical: ScrollBar {}

            delegate: AbstractButton {
                id: row
                required property var modelData
                width: ListView.view.width - 20
                implicitHeight: 44
                hoverEnabled: true
                onClicked: column.selectedId = modelData.id

                background: Rectangle {
                    radius: Theme.rowRadius
                    color: column.selectedId === row.modelData.id ? Theme.selection
                         : row.hovered ? Theme.hover : "transparent"
                }

                contentItem: RowLayout {
                    spacing: 10

                    Avatar {
                        Layout.leftMargin: 6
                        name: row.modelData.name
                        photo: row.modelData.photo || ""
                        size: 30
                    }

                    Text {
                        Layout.fillWidth: true
                        text: row.modelData.name
                        color: Theme.text
                        font.pixelSize: Theme.bodySize + 1
                        elide: Text.ElideRight
                    }
                }
            }

            EmptyState {
                anchors.centerIn: parent
                width: parent.width - 40
                visible: column.shown.length === 0
                iconName: "person"
                title: L10n.t("No Contacts")
            }
        }
    }
}
