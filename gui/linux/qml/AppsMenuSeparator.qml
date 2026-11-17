import QtQuick
import QtQuick.Controls.Basic
import PhoneLink

// The line between groups of rows in an AppsMenu; an invisible one takes no room.
MenuSeparator {
    height: visible ? implicitHeight : 0
    topPadding: 4
    bottomPadding: 4
    leftPadding: 6
    rightPadding: 6

    contentItem: Rectangle {
        implicitWidth: 150
        implicitHeight: 1
        color: Theme.separator
    }
}
