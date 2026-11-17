import QtQuick
import QtQuick.Layouts
import PhoneLink

// A titled group of settings rows in a settings pane.
Rectangle {
    default property alias rows: column.data
    property string title

    Layout.fillWidth: true
    implicitHeight: column.implicitHeight + 32
    radius: Theme.moduleRadius
    color: Theme.listBackground
    border.color: Theme.separator

    Text {
        x: 4
        y: -20
        text: parent.title
        color: Theme.secondaryText
        font.pixelSize: Theme.subheadSize
        font.weight: Font.DemiBold
    }

    ColumnLayout {
        id: column
        anchors.fill: parent
        anchors.margins: 16
        spacing: 12
    }
}
