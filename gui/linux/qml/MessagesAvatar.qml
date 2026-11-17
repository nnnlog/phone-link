import QtQuick
import PhoneLink

// A conversation's picture: the person's contact photo or monogram, or a group's: the shared
// monogram's gray gradient with two people on it.
Item {
    id: avatar

    property bool group: false
    property string name
    property string photo
    property real size: 40

    implicitWidth: size
    implicitHeight: size

    Avatar {
        anchors.fill: parent
        visible: !avatar.group
        name: avatar.name
        photo: avatar.photo
        size: avatar.size
    }

    Rectangle {
        anchors.fill: parent
        visible: avatar.group
        radius: width / 2
        gradient: Gradient {
            GradientStop { position: 0; color: Qt.rgba(0.65, 0.67, 0.72, 1) }
            GradientStop { position: 1; color: Qt.rgba(0.52, 0.54, 0.59, 1) }
        }

        Icon {
            anchors.centerIn: parent
            name: "people"
            filled: true
            color: "white"
            size: avatar.size * 0.55
        }
    }
}
