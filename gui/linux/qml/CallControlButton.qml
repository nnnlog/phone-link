import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import PhoneLink

// One of the call window's in-call controls: a round button with its icon, and its name under it. It
// is filled while `active` (the call is muted, on hold, the keypad shows).
AbstractButton {
    id: control

    property string iconName
    property bool active: false
    property string help

    hoverEnabled: true
    focusPolicy: Qt.StrongFocus
    implicitWidth: 64
    implicitHeight: column.implicitHeight
    opacity: enabled ? 1 : 0.4

    Accessible.role: Accessible.Button
    Accessible.name: text
    Accessible.description: help
    ToolTip.visible: hovered && help.length > 0
    ToolTip.delay: 600
    ToolTip.text: help

    background: null

    contentItem: Column {
        id: column

        spacing: 5

        Rectangle {
            anchors.horizontalCenter: parent.horizontalCenter
            width: 44
            height: 44
            radius: 22
            color: control.active ? Theme.text
                 : control.down ? Theme.selection
                 : control.hovered ? Theme.controlHover : Theme.controlBackground
            border.width: control.visualFocus ? 2 : 0
            border.color: Theme.accent

            Icon {
                anchors.centerIn: parent
                name: control.iconName
                size: 18
                color: control.active ? Theme.windowBackground : Theme.text
            }
        }

        Text {
            width: control.implicitWidth
            horizontalAlignment: Text.AlignHCenter
            text: control.text
            color: Theme.secondaryText
            font.pixelSize: Theme.captionSize
            elide: Text.ElideRight
        }
    }
}
