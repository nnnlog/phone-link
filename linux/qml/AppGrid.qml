import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import PhoneLink

Rectangle {
    id: column

    readonly property var apps: { EngineBridge.appsRevision; return JSON.parse(EngineBridge.query("apps", "")) || [] }
    readonly property var shown: {
        const query = search.text.trim().toLowerCase()
        const list = apps.slice().sort((a, b) => (b.favorite - a.favorite) || a.name.localeCompare(b.name))
        return query.length === 0 ? list : list.filter(a => a.name.toLowerCase().includes(query))
    }

    color: Theme.windowBackground

    ColumnLayout {
        anchors.fill: parent
        spacing: 0

        RowLayout {
            Layout.fillWidth: true
            Layout.preferredHeight: 52
            Layout.leftMargin: 20
            Layout.rightMargin: 14

            Text {
                Layout.fillWidth: true
                text: L10n.t("Apps")
                color: Theme.text
                font.pixelSize: Theme.headlineSize
                font.weight: Font.Bold
            }

            CapsuleField {
                id: search
                Layout.preferredWidth: 260
                placeholderText: L10n.t("Search")
                submittable: false
            }
        }

        GridView {
            id: grid
            Layout.fillWidth: true
            Layout.fillHeight: true
            Layout.leftMargin: 16
            clip: true
            model: column.shown
            cellWidth: 104
            cellHeight: 104
            ScrollBar.vertical: ScrollBar {}

            delegate: AbstractButton {
                id: tile
                required property var modelData
                width: grid.cellWidth
                height: grid.cellHeight
                hoverEnabled: true
                onClicked: EngineBridge.perform("mirror.launch", tile.modelData["package"])

                background: Rectangle {
                    anchors.fill: parent
                    anchors.margins: 4
                    radius: Theme.rowRadius
                    color: tile.hovered ? Theme.hover : "transparent"
                }

                ColumnLayout {
                    anchors.centerIn: parent
                    width: parent.width - 12
                    spacing: 6

                    AppGlyph {
                        Layout.alignment: Qt.AlignHCenter
                        name: tile.modelData.name
                        packageName: tile.modelData.icon ? tile.modelData["package"] : ""
                        size: 52
                    }

                    Text {
                        Layout.fillWidth: true
                        text: tile.modelData.name
                        color: Theme.text
                        font.pixelSize: Theme.captionSize + 1
                        horizontalAlignment: Text.AlignHCenter
                        elide: Text.ElideRight
                    }
                }
            }

            EmptyState {
                anchors.centerIn: parent
                width: parent.width - 40
                visible: column.shown.length === 0
                iconName: "apps"
                title: L10n.t("No Apps")
            }
        }
    }
}
