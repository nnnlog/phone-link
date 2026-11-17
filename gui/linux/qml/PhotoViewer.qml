import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import PhoneLink

// The viewer over the gallery: the focused photo as large as it fits, its full image once it is on
// this computer and its thumbnail until then (also when the full image is not a picture Qt can read).
// The arrow keys and the side buttons move to the neighboring photos, Space and Escape close it.
Rectangle {
    id: viewer

    // The photo shown; -1 while the viewer is closed.
    property double photoId: -1
    // The grid's columns, for the up and down arrows.
    property int columns: 1
    property var shown: null
    readonly property bool open: photoId >= 0
    readonly property bool waitingForFull: open && shown !== null
        && (!shown.full || fullImage.status === Image.Loading)

    signal moveRequested(int offset)
    signal closeRequested()
    signal openRequested()
    signal copyRequested()
    signal saveRequested()

    function reload() {
        shown = photoId >= 0 ? JSON.parse(EngineBridge.query("photos.viewer", String(photoId))) : null
    }

    color: Qt.rgba(0, 0, 0, 0.94)
    visible: open
    onPhotoIdChanged: reload()
    // The viewer takes the keyboard focus, so its keys reach it.
    onVisibleChanged: {
        if (visible)
            forceActiveFocus()
    }

    Keys.onPressed: event => {
        const control = (event.modifiers & Qt.ControlModifier) !== 0
        switch (event.key) {
        case Qt.Key_Left: viewer.moveRequested(-1); break
        case Qt.Key_Right: viewer.moveRequested(1); break
        case Qt.Key_Up: viewer.moveRequested(-viewer.columns); break
        case Qt.Key_Down: viewer.moveRequested(viewer.columns); break
        case Qt.Key_Space:
        case Qt.Key_Escape: viewer.closeRequested(); break
        case Qt.Key_C:
            if (!control)
                return
            viewer.copyRequested()
            break
        default:
            return
        }
        event.accepted = true
    }

    Connections {
        target: EngineBridge
        function onPhotosRevisionChanged() { viewer.reload() }
        function onAreaEvent(area, name, payload) {
            if (area === "photos" && name === "fullImage" && JSON.parse(payload).id === viewer.photoId)
                viewer.reload()
        }
    }

    MouseArea {
        anchors.fill: parent
        onClicked: viewer.closeRequested()
    }

    Image {
        anchors.fill: parent
        anchors.margins: 56
        visible: fullImage.status !== Image.Ready
        source: viewer.shown && viewer.shown.thumbnail ? viewer.shown.thumbnail : ""
        fillMode: Image.PreserveAspectFit
        asynchronous: true
    }

    Image {
        id: fullImage
        anchors.fill: parent
        anchors.margins: 56
        source: viewer.shown && viewer.shown.full ? viewer.shown.full : ""
        sourceSize.width: Math.max(1, Math.ceil(width * Screen.devicePixelRatio))
        sourceSize.height: Math.max(1, Math.ceil(height * Screen.devicePixelRatio))
        fillMode: Image.PreserveAspectFit
        asynchronous: true
    }

    BusyIndicator {
        anchors.right: parent.right
        anchors.bottom: parent.bottom
        anchors.margins: 16
        width: 28
        height: 28
        padding: 2
        running: viewer.waitingForFull
        visible: running
        palette.dark: "white"
    }

    IconButton {
        anchors.left: parent.left
        anchors.leftMargin: 12
        anchors.verticalCenter: parent.verticalCenter
        iconName: "chevronLeft"
        label: L10n.t("Previous Photo")
        onDark: true
        onClicked: viewer.moveRequested(-1)
    }

    IconButton {
        anchors.right: parent.right
        anchors.rightMargin: 12
        anchors.verticalCenter: parent.verticalCenter
        iconName: "chevronRight"
        label: L10n.t("Next Photo")
        onDark: true
        onClicked: viewer.moveRequested(1)
    }

    RowLayout {
        anchors.top: parent.top
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.margins: 12
        spacing: 8

        ColumnLayout {
            Layout.fillWidth: true
            Layout.leftMargin: 8
            spacing: 0

            Text {
                Layout.fillWidth: true
                text: viewer.shown ? viewer.shown.title : ""
                color: "white"
                font.pixelSize: Theme.bodySize
                font.weight: Font.DemiBold
                elide: Text.ElideMiddle
            }

            Text {
                Layout.fillWidth: true
                text: viewer.shown ? Dates.detailStamp(viewer.shown.takenMs) : ""
                color: Qt.rgba(1, 1, 1, 0.7)
                font.pixelSize: Theme.subheadSize
                elide: Text.ElideRight
            }
        }

        IconButton {
            iconName: "openExternal"
            label: L10n.t("Open")
            onDark: true
            onClicked: viewer.openRequested()
        }

        IconButton {
            iconName: "copy"
            label: L10n.t("Copy")
            onDark: true
            onClicked: viewer.copyRequested()
        }

        IconButton {
            iconName: "save"
            label: L10n.t("Save As...")
            onDark: true
            onClicked: viewer.saveRequested()
        }

        IconButton {
            iconName: "close"
            label: L10n.t("Close")
            onDark: true
            onClicked: viewer.closeRequested()
        }
    }
}
