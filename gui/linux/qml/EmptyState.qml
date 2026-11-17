import QtQuick
import QtQuick.Layouts

ColumnLayout {
    id: state

    property string iconName
    property string title
    property string description

    spacing: 8

    Icon {
        Layout.alignment: Qt.AlignHCenter
        visible: state.iconName.length > 0
        name: state.iconName
        size: 40
        color: Theme.secondaryText
    }

    Text {
        Layout.fillWidth: true
        text: state.title
        color: Theme.text
        font.pixelSize: Theme.headlineSize
        font.weight: Font.Bold
        horizontalAlignment: Text.AlignHCenter
        wrapMode: Text.Wrap
    }

    Text {
        Layout.fillWidth: true
        visible: text.length > 0
        text: state.description
        color: Theme.secondaryText
        font.pixelSize: Theme.bodySize
        horizontalAlignment: Text.AlignHCenter
        wrapMode: Text.Wrap
    }
}
