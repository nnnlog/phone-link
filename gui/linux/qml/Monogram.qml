import QtQuick

Rectangle {
    id: monogram

    property string name
    property real size: 32
    readonly property string initials: initialsOf(name)

    function initialsOf(text) {
        const trimmed = (text || "").trim()
        if (trimmed.length === 0 || /^[+0-9 \-()#*]+$/.test(trimmed))
            return ""
        const words = trimmed.split(/\s+/)
        if (words.length > 1)
            return (words[0][0] + words[words.length - 1][0]).toUpperCase()
        if (/[一-鿿가-힣]/.test(words[0]))
            return words[0].slice(-2)
        return words[0][0].toUpperCase()
    }

    width: size
    height: size
    radius: size / 2
    gradient: Gradient {
        GradientStop { position: 0; color: Qt.rgba(0.65, 0.67, 0.72, 1) }
        GradientStop { position: 1; color: Qt.rgba(0.52, 0.54, 0.59, 1) }
    }

    Text {
        anchors.centerIn: parent
        visible: monogram.initials.length > 0
        text: monogram.initials
        color: "white"
        font.pixelSize: monogram.size * 0.38
        font.weight: Font.Medium
    }

    Icon {
        anchors.centerIn: parent
        visible: monogram.initials.length === 0
        name: "personFill"
        filled: true
        color: "white"
        size: monogram.size * 0.55
    }
}
