import QtQuick
import QtQuick.Controls.Basic
import PhoneLink

// A row of an AppsMenu: its text, a submenu's arrow, the accent behind it while highlighted. A
// destructive row (Delete) draws its text in red. An invisible row takes no room.
MenuItem {
    id: item

    property bool destructive: false

    height: visible ? implicitHeight : 0
    implicitHeight: 26
    leftPadding: 10
    rightPadding: 10
    font.pixelSize: Theme.bodySize

    contentItem: Text {
        rightPadding: item.subMenu ? 18 : 0
        text: item.text
        font: item.font
        color: item.highlighted ? "white" : item.destructive ? Theme.destructive : Theme.text
        opacity: item.enabled ? 1 : 0.4
        elide: Text.ElideRight
        verticalAlignment: Text.AlignVCenter
    }

    indicator: Item {}

    arrow: Icon {
        x: item.width - width - 6
        y: (item.height - height) / 2
        visible: item.subMenu !== null
        name: "chevronRight"
        size: 12
        color: item.highlighted ? "white" : Theme.secondaryText
    }

    background: Rectangle {
        implicitWidth: 160
        radius: Theme.rowRadius - 2
        color: item.highlighted && item.enabled ? Theme.accent : "transparent"
    }
}
