import QtQuick
import PhoneLink

Item {
    id: glyph

    property string name
    property string packageName
    property real size: 16

    readonly property var palette: ["#ff9f0a", "#30d158", "#0a84ff", "#bf5af2", "#ff453a", "#64d2ff", "#ffd60a", "#5e5ce6"]
    readonly property string iconUrl: packageName.length > 0 ? JSON.parse(EngineBridge.query("appIcon", packageName)) || "" : ""

    function hash(text) {
        let value = 0
        for (let i = 0; i < text.length; i++)
            value = (value * 31 + text.charCodeAt(i)) % 9973
        return value
    }

    width: size
    height: size

    Image {
        anchors.fill: parent
        visible: glyph.iconUrl.length > 0
        source: glyph.iconUrl
        sourceSize.width: glyph.size * 2
        sourceSize.height: glyph.size * 2
        fillMode: Image.PreserveAspectFit
        smooth: true
    }

    Rectangle {
        anchors.fill: parent
        visible: glyph.iconUrl.length === 0
        radius: glyph.size * 0.25
        color: glyph.palette[glyph.hash(glyph.name || "") % glyph.palette.length]

        Text {
            anchors.centerIn: parent
            text: (glyph.name || "?").trim().charAt(0).toUpperCase()
            color: "white"
            font.pixelSize: glyph.size * 0.6
            font.weight: Font.Bold
        }
    }
}
