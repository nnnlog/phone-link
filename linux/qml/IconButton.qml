import QtQuick
import QtQuick.Controls.Basic

AbstractButton {
    id: control

    property string iconName
    property bool active: false

    implicitWidth: 32
    implicitHeight: 32
    hoverEnabled: true

    background: Rectangle {
        radius: width / 2
        color: control.active ? Theme.accent : control.hovered ? Theme.controlHover : Theme.controlBackground
        opacity: control.down ? 0.75 : 1
    }

    contentItem: Item {
        Icon {
            anchors.centerIn: parent
            name: control.iconName
            size: 16
            color: control.active ? "white" : Theme.text
            opacity: control.enabled ? 1 : 0.4
        }
    }
}
