import QtQuick
import QtQuick.Controls.Basic
import PhoneLink

// A line between groups of a CallMenu's entries.
MenuSeparator {
    topPadding: 4
    bottomPadding: 4
    leftPadding: 6
    rightPadding: 6

    contentItem: Rectangle {
        implicitHeight: 1
        color: Theme.separator
    }
}
