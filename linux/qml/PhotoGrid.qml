import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Dialogs
import QtQuick.Layouts
import PhoneLink

Rectangle {
    id: column

    property var viewing: null
    readonly property string viewingSource: viewing ? JSON.parse(EngineBridge.query("photo", String(viewing.id))) || viewing.thumbnail || "" : ""

    function entry(photo) {
        return { photoId: photo.id, name: photo.name, takenMs: photo.takenMs, thumbnail: photo.thumbnail || "" }
    }

    function sync() {
        const list = JSON.parse(EngineBridge.query("photos", "")) || []
        const sameItems = list.length === photoModel.count
            && list.every((photo, index) => photoModel.get(index).photoId === photo.id)
        if (!sameItems) {
            photoModel.clear()
            list.forEach(photo => photoModel.append(entry(photo)))
        } else {
            list.forEach((photo, index) => {
                const thumbnail = photo.thumbnail || ""
                if (photoModel.get(index).thumbnail !== thumbnail)
                    photoModel.setProperty(index, "thumbnail", thumbnail)
            })
        }
        layoutTimer.restart()
    }

    function reportVisible() {
        if (!column.visible || photoModel.count === 0)
            return
        const first = Math.max(0, grid.indexAt(4, grid.contentY + 4))
        let last = grid.indexAt(grid.width - 8, grid.contentY + grid.height - 4)
        if (last < 0)
            last = photoModel.count - 1
        const ids = []
        for (let index = first; index <= last; index++)
            ids.push(photoModel.get(index).photoId)
        EngineBridge.perform("photos.layout", JSON.stringify(ids))
    }

    color: Theme.windowBackground

    onVisibleChanged: visible ? sync() : EngineBridge.perform("photos.left", "")
    Component.onCompleted: sync()

    ListModel {
        id: photoModel
    }

    Connections {
        target: EngineBridge
        function onPhotosRevisionChanged() { column.sync() }
    }

    Timer {
        id: layoutTimer
        interval: 250
        onTriggered: column.reportVisible()
    }

    FileDialog {
        id: saveDialog
        fileMode: FileDialog.SaveFile
        currentFile: column.viewing ? column.viewing.name : ""
        onAccepted: EngineBridge.perform("photos.save", JSON.stringify({ id: column.viewing.id, path: selectedFile.toString() }))
    }

    ColumnLayout {
        anchors.fill: parent
        spacing: 0

        Text {
            Layout.fillWidth: true
            Layout.preferredHeight: 52
            Layout.leftMargin: 20
            verticalAlignment: Text.AlignVCenter
            text: L10n.t("Photos")
            color: Theme.text
            font.pixelSize: Theme.headlineSize
            font.weight: Font.Bold
        }

        GridView {
            id: grid
            Layout.fillWidth: true
            Layout.fillHeight: true
            Layout.leftMargin: 16
            clip: true
            model: photoModel
            readonly property int columns: Math.max(3, Math.floor(width / 170))
            cellWidth: (width - 4) / columns
            cellHeight: cellWidth
            ScrollBar.vertical: ScrollBar {}
            onContentYChanged: layoutTimer.restart()
            onHeightChanged: layoutTimer.restart()

            delegate: AbstractButton {
                id: tile
                required property var photoId
                required property string name
                required property double takenMs
                required property string thumbnail
                width: grid.cellWidth
                height: grid.cellHeight
                hoverEnabled: true
                onClicked: column.viewing = { id: photoId, name: name, takenMs: takenMs, thumbnail: thumbnail }
                Component.onCompleted: {
                    if (thumbnail.length === 0)
                        EngineBridge.perform("photos.shown", String(photoId))
                }
                Component.onDestruction: {
                    if (thumbnail.length === 0)
                        EngineBridge.perform("photos.hidden", String(photoId))
                }

                contentItem: Rectangle {
                    anchors.fill: parent
                    anchors.margins: 2
                    radius: 4
                    color: Theme.controlBackground
                    clip: true

                    Image {
                        anchors.fill: parent
                        source: tile.thumbnail
                        fillMode: Image.PreserveAspectCrop
                        sourceSize.width: 340
                        sourceSize.height: 340
                        asynchronous: true
                    }

                    Rectangle {
                        anchors.fill: parent
                        color: "white"
                        opacity: tile.hovered ? 0.08 : 0
                    }
                }
            }

            EmptyState {
                anchors.centerIn: parent
                width: parent.width - 40
                visible: grid.count === 0
                iconName: "photo"
                title: L10n.t("No Photos")
            }
        }
    }

    Rectangle {
        anchors.fill: parent
        visible: column.viewing !== null
        color: Qt.rgba(0, 0, 0, 0.88)

        MouseArea {
            anchors.fill: parent
            onClicked: column.viewing = null
        }

        Image {
            anchors.fill: parent
            anchors.margins: 48
            source: column.viewingSource
            fillMode: Image.PreserveAspectFit
            asynchronous: true
        }

        Row {
            anchors.top: parent.top
            anchors.right: parent.right
            anchors.margins: 12
            spacing: 8

            IconButton {
                iconName: "save"
                onClicked: saveDialog.open()
            }

            IconButton {
                iconName: "close"
                onClicked: column.viewing = null
            }
        }

        Text {
            anchors.bottom: parent.bottom
            anchors.horizontalCenter: parent.horizontalCenter
            anchors.bottomMargin: 16
            text: column.viewing ? column.viewing.name + "  " + Dates.detailStamp(column.viewing.takenMs) : ""
            color: "white"
            font.pixelSize: Theme.subheadSize
        }
    }
}
