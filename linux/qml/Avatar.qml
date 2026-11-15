import QtQuick
import QtQuick.Effects

Item {
    id: avatar

    property string name
    property string photo
    property real size: 32

    width: size
    height: size

    Monogram {
        anchors.fill: parent
        visible: avatar.photo.length === 0 || image.status !== Image.Ready
        name: avatar.name
        size: avatar.size
    }

    Image {
        id: image
        anchors.fill: parent
        visible: false
        source: avatar.photo
        sourceSize.width: avatar.size * 2
        sourceSize.height: avatar.size * 2
        fillMode: Image.PreserveAspectCrop
    }

    Rectangle {
        id: mask
        anchors.fill: parent
        radius: width / 2
        visible: false
        layer.enabled: true
    }

    MultiEffect {
        anchors.fill: parent
        visible: image.status === Image.Ready
        source: image
        maskEnabled: true
        maskSource: mask
    }
}
