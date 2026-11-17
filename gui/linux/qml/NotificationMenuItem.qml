import QtQuick
import QtQuick.Controls.Basic
import PhoneLink

// A command in a menu of the notifications part: red when it is destructive, a check mark when it is the
// current choice, an arrow when it opens a submenu.
MenuItem {
    id: item

    property bool destructive: false
    property bool chosen: false

    implicitHeight: 26
    leftPadding: 26
    rightPadding: subMenu ? 30 : 16
    font.pixelSize: Theme.bodySize

    contentItem: Text {
        text: item.text
        font: item.font
        color: item.highlighted && item.enabled ? "white" : item.destructive ? Theme.destructive : Theme.text
        opacity: item.enabled ? 1 : 0.4
        elide: Text.ElideRight
        verticalAlignment: Text.AlignVCenter
    }

    indicator: Icon {
        x: 8
        y: (item.height - height) / 2
        visible: item.chosen
        name: "checkmark"
        size: 12
        color: item.highlighted ? "white" : Theme.text
    }

    arrow: Icon {
        x: item.width - width - 10
        y: (item.height - height) / 2
        visible: item.subMenu !== null
        name: "chevronRight"
        size: 12
        color: item.highlighted ? "white" : Theme.secondaryText
    }

    background: Rectangle {
        x: 5
        width: item.width - 10
        height: item.height
        radius: 5
        color: item.highlighted && item.enabled ? Theme.accent : "transparent"
    }
}
