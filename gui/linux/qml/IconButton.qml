import QtQuick
import QtQuick.Controls.Basic

AbstractButton {
    id: control

    property string iconName
    property string label
    property bool active: false
    // Drawn over a dark backdrop (the photo viewer) in either theme.
    property bool onDark: false

    implicitWidth: 32
    implicitHeight: 32
    hoverEnabled: true

    Accessible.role: Accessible.Button
    Accessible.name: label
    ToolTip.visible: hovered && label.length > 0
    ToolTip.delay: 600
    ToolTip.text: label

    background: Rectangle {
        radius: width / 2
        color: control.active ? Theme.accent
             : control.onDark ? Qt.rgba(1, 1, 1, control.hovered ? 0.26 : 0.16)
             : control.hovered ? Theme.controlHover : Theme.controlBackground
        opacity: control.down ? 0.75 : 1
    }

    contentItem: Item {
        Icon {
            anchors.centerIn: parent
            name: control.iconName
            size: 16
            color: control.active || control.onDark ? "white" : Theme.text
            opacity: control.enabled ? 1 : 0.4
        }
    }
}
