import QtQuick
import QtQuick.Controls.Basic
import PhoneLink

// One photo of the grid: its picture cropped square, a neutral fill until it loads, the accent ring when
// selected, a spinner while an action waits on the phone, dimmed while it is being deleted. A click
// selects it (Ctrl and Shift as in a file manager), a double-click opens the viewer, a right-click
// opens its menu, and it drags out as a file.
Item {
    id: tile

    required property string json
    readonly property var photo: JSON.parse(json)
    readonly property double photoId: photo.id
    property bool selected: false
    property bool busy: false
    // Thumbnail, or the full image the view scales down when the photo has no thumbnail.
    readonly property string picture: photo.hasThumbnail || photo.hasFull
        ? JSON.parse(EngineBridge.query("photos.thumbnail", photo.id + ":" + photo.modifiedMs)) || "" : ""

    signal tapped(int modifiers)
    signal doubleTapped()
    // A right-click at (x, y) in the tile.
    signal menuRequested(real x, real y)
    signal dragRequested()

    Accessible.role: Accessible.Graphic
    Accessible.name: photo.name
    Accessible.selectable: true
    Accessible.selected: selected

    opacity: photo.deleteState === "deleting" ? 0.4 : photo.deleteState === "deleted" ? 0 : 1

    Behavior on opacity {
        NumberAnimation { duration: 200 }
    }

    // The engine fetches the thumbnail of a cell on screen, and cancels it when the cell goes.
    Component.onCompleted: EngineBridge.perform("photos.shown", String(photoId))
    Component.onDestruction: EngineBridge.perform("photos.hidden", String(photoId))

    Rectangle {
        anchors.fill: parent
        anchors.margins: 1
        color: Theme.controlBackground
        clip: true

        Image {
            anchors.fill: parent
            source: tile.picture
            fillMode: Image.PreserveAspectCrop
            sourceSize.width: 360
            sourceSize.height: 360
            asynchronous: true
        }

        Rectangle {
            anchors.fill: parent
            color: "white"
            opacity: mouse.containsMouse ? 0.08 : 0
        }
    }

    Rectangle {
        anchors.fill: parent
        anchors.margins: 1
        visible: tile.selected
        color: "transparent"
        border.color: Theme.accent
        border.width: 3
    }

    Rectangle {
        anchors.centerIn: parent
        width: 34
        height: 34
        radius: 17
        visible: tile.busy
        color: Qt.rgba(0, 0, 0, 0.45)

        BusyIndicator {
            anchors.centerIn: parent
            width: 26
            height: 26
            padding: 2
            running: tile.busy
            palette.dark: "white"
        }
    }

    MouseArea {
        id: mouse

        property point pressedAt
        property bool dragTried: false

        anchors.fill: parent
        acceptedButtons: Qt.LeftButton | Qt.RightButton
        hoverEnabled: true
        onPressed: event => {
            pressedAt = Qt.point(event.x, event.y)
            dragTried = false
            if (event.button === Qt.RightButton)
                tile.menuRequested(event.x, event.y)
        }
        onClicked: event => {
            if (event.button === Qt.LeftButton)
                tile.tapped(event.modifiers)
        }
        onDoubleClicked: event => {
            if (event.button === Qt.LeftButton)
                tile.doubleTapped()
        }
        onPositionChanged: event => {
            if (!(event.buttons & Qt.LeftButton) || dragTried)
                return
            if (Math.abs(event.x - pressedAt.x) + Math.abs(event.y - pressedAt.y) < Qt.styleHints.startDragDistance)
                return
            dragTried = true
            tile.dragRequested()
        }
    }
}
