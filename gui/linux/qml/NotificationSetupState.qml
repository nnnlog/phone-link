import QtQuick
import QtQuick.Layouts
import PhoneLink

// What the notification list column shows while there is no list to show: why, and the action that
// sets it up, when there is one.
ColumnLayout {
    id: setup

    property string iconName
    property string title
    property string description
    property string actionText
    property bool actionEnabled: true
    property bool prominent: true

    signal actionTriggered()

    spacing: 14

    EmptyState {
        Layout.fillWidth: true
        iconName: setup.iconName
        title: setup.title
        description: setup.description
    }

    PillButton {
        Layout.alignment: Qt.AlignHCenter
        Layout.maximumWidth: setup.width
        visible: setup.actionText.length > 0
        prominent: setup.prominent && setup.actionEnabled
        enabled: setup.actionEnabled
        text: setup.actionText
        onClicked: setup.actionTriggered()
    }
}
