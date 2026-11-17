import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import PhoneLink

// A call button in a solid color with white text: red to decline or end a call, green to answer or
// place one. It keeps its color while its window is not active.
AbstractButton {
    id: control

    property color fill
    property string iconName
    property bool iconFilled: true
    property string help

    hoverEnabled: true
    focusPolicy: Qt.StrongFocus
    implicitWidth: content.implicitWidth + 32
    implicitHeight: 34

    Accessible.role: Accessible.Button
    Accessible.name: text
    Accessible.description: help
    ToolTip.visible: hovered && help.length > 0
    ToolTip.delay: 600
    ToolTip.text: help

    background: Rectangle {
        radius: height / 2
        color: control.fill
        opacity: !control.enabled ? 0.45 : control.down ? 0.7 : control.hovered ? 0.9 : 1
        border.width: control.visualFocus ? 2 : 0
        border.color: Theme.text
    }

    contentItem: Item {
        RowLayout {
            id: content

            anchors.centerIn: parent
            spacing: 6

            Icon {
                visible: control.iconName.length > 0
                name: control.iconName
                filled: control.iconFilled
                size: 15
                color: "white"
            }

            Text {
                text: control.text
                color: "white"
                font.pixelSize: Theme.bodySize + 1
                font.weight: Font.DemiBold
                elide: Text.ElideRight
            }
        }
    }
}
