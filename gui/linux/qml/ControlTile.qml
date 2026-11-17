import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts

// One of the phone's controls in the sidebar: a circle with its symbol (filled with the accent color
// while on, progress while a change is under way), its name and its state.
AbstractButton {
    id: tile

    property string iconName
    property string title
    property string subtitle
    property bool on: false
    property bool busy: false

    implicitHeight: 44
    leftPadding: Theme.content - Theme.edge
    rightPadding: 8
    hoverEnabled: true
    opacity: enabled || busy || on ? 1 : 0.5

    Accessible.role: Accessible.Button
    Accessible.name: title
    Accessible.description: subtitle

    background: Rectangle {
        radius: Theme.moduleRadius
        color: tile.hovered && tile.enabled ? Theme.controlHover : Theme.sidebarModule
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
                visible: !tile.busy
                name: tile.iconName
                size: 14
                color: tile.on ? "white" : Theme.text
            }

            BusyIndicator {
                anchors.centerIn: parent
                width: 18
                height: 18
                padding: 0
                visible: tile.busy
                running: visible
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
