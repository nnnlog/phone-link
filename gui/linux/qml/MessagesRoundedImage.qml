import QtQuick
import QtQuick.Effects

// A picture with rounded corners: a message's image or a draft's thumbnail. `ready` once it shows.
Item {
    id: rounded

    property alias source: picture.source
    property alias sourceSize: picture.sourceSize
    property alias fillMode: picture.fillMode
    property real radius: 14
    readonly property bool ready: picture.status === Image.Ready
    readonly property real pictureWidth: picture.implicitWidth
    readonly property real pictureHeight: picture.implicitHeight

    Image {
        id: picture
        anchors.fill: parent
        visible: false
        asynchronous: true
        fillMode: Image.PreserveAspectCrop
    }

    Rectangle {
        id: mask
        anchors.fill: parent
        radius: rounded.radius
        visible: false
        layer.enabled: true
    }

    MultiEffect {
        anchors.fill: parent
        visible: rounded.ready
        source: picture
        maskEnabled: true
        maskSource: mask
    }
}
