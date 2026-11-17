import QtQuick
import QtQuick.Layouts
import PhoneLink

// A titled group of app tiles in the Apps section, its hint while it holds none. An app dropped on the
// group (not on one of its tiles) goes to `appDropped` when the group takes drops.
Item {
    id: group

    property string title
    property string hint
    property alias model: repeater.model
    property alias delegate: repeater.delegate
    property bool acceptsDrops: false

    signal appDropped(string packageName)

    implicitHeight: layout.implicitHeight

    Rectangle {
        anchors.fill: parent
        anchors.margins: -6
        radius: Theme.moduleRadius
        visible: drop.containsDrag
        color: Theme.hover
        border.color: Theme.separator
    }

    DropArea {
        id: drop
        anchors.fill: parent
        enabled: group.acceptsDrops
        keys: ["application/x-phonelink-app"]
        onDropped: event => {
            const dropped = event.getDataAsString("application/x-phonelink-app")
            if (dropped.length === 0)
                return
            group.appDropped(dropped)
            event.accept(Qt.MoveAction)
        }
    }

    ColumnLayout {
        id: layout
        width: parent.width
        spacing: 8

        Text {
            Layout.fillWidth: true
            text: group.title
            color: Theme.text
            font.pixelSize: Theme.bodySize + 1
            font.weight: Font.DemiBold
            Accessible.role: Accessible.Heading
        }

        Text {
            Layout.fillWidth: true
            Layout.minimumHeight: 48
            visible: repeater.count === 0
            text: group.hint
            color: Theme.secondaryText
            font.pixelSize: Theme.bodySize
            verticalAlignment: Text.AlignVCenter
            wrapMode: Text.Wrap
        }

        Flow {
            Layout.fillWidth: true
            visible: repeater.count > 0
            spacing: 4

            Repeater {
                id: repeater
            }
        }
    }
}
