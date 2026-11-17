import QtQuick
import QtQuick.Controls.Basic
import PhoneLink

// An entry of a CallMenu. A hidden entry takes no room in its menu.
MenuItem {
    id: item

    implicitHeight: visible ? 26 : 0
    height: visible ? implicitHeight : 0
    leftPadding: 10
    rightPadding: 10
    font.pixelSize: Theme.bodySize

    contentItem: Text {
        text: item.text
        font: item.font
        color: item.highlighted ? "white" : Theme.text
        opacity: item.enabled ? 1 : 0.4
        elide: Text.ElideRight
        verticalAlignment: Text.AlignVCenter
    }

    background: Rectangle {
        radius: 5
        color: item.highlighted ? Theme.accent : "transparent"
    }
}
