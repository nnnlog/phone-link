import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import PhoneLink

// One tab of the Settings window: a scrolling column of SettingsSection items.
Flickable {
    id: pane

    default property alias sections: content.data

    contentHeight: content.implicitHeight + 60
    clip: true
    ScrollBar.vertical: ScrollBar {}

    ColumnLayout {
        id: content
        x: 24
        y: 40
        width: pane.width - 48
        spacing: 40
    }
}
