import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts

AbstractButton {
    id: tile

    property string iconName
    property string title
    property string subtitle
    property bool on: false

    implicitHeight: 44
    leftPadding: Theme.content - Theme.edge
    rightPadding: 8
    hoverEnabled: true
    opacity: enabled ? 1 : 0.5

    background: Rectangle {
        radius: Theme.moduleRadius
        color: tile.hovered ? Theme.controlHover : Theme.sidebarModule
    }

    contentItem: RowLayout {
        spacing: 6

        Rectangle {
            Layout.preferredWidth: 26
            Layout.preferredHeight: 26
            radius: 13
            color: tile.on ? Theme.accent : Theme.controlBackground

            Icon {
                anchors.centerIn: parent
                name: tile.iconName
                size: 14
                color: tile.on ? "white" : Theme.text
            }
        }

        ColumnLayout {
            Layout.fillWidth: true
            spacing: 0

            Text {
                Layout.fillWidth: true
                text: tile.title
                color: Theme.text
                font.pixelSize: Theme.bodySize
                font.weight: Font.DemiBold
                elide: Text.ElideRight
            }

            Text {
                Layout.fillWidth: true
                visible: tile.subtitle.length > 0
                text: tile.subtitle
                color: Theme.secondaryText
                font.pixelSize: Theme.captionSize
                elide: Text.ElideRight
            }
        }
    }
}
