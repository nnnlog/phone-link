import QtQuick
import QtQuick.Controls.Basic

Button {
    id: control

    property bool prominent: false
    property bool destructive: false

    leftPadding: 12
    rightPadding: 12
    topPadding: 5
    bottomPadding: 5
    font.pixelSize: Theme.bodySize

    contentItem: Text {
        text: control.text
        font: control.font
        color: control.prominent ? "white" : control.destructive ? Theme.destructive : Theme.text
        opacity: control.enabled ? 1 : 0.4
        horizontalAlignment: Text.AlignHCenter
        verticalAlignment: Text.AlignVCenter
        elide: Text.ElideRight
    }

    background: Rectangle {
        implicitHeight: 28
        radius: height / 2
        color: control.prominent ? (control.destructive ? Theme.destructive : Theme.accent)
             : control.hovered ? Theme.controlHover : Theme.controlBackground
        opacity: control.down ? 0.75 : 1
    }
}
