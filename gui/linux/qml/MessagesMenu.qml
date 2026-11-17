import QtQuick
import QtQuick.Controls.Basic
import PhoneLink

// A menu of the Messages section in the app's look: a raised, rounded panel.
Menu {
    topPadding: 4
    bottomPadding: 4

    background: Rectangle {
        implicitWidth: 200
        radius: 8
        color: Theme.raisedBackground
        border.color: Theme.separator
    }
}
