import QtCore
import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Dialogs
import QtQuick.Layouts
import PhoneLink

// The Photos section: the phone's recent photos as a grid, or the one state that keeps them from
// showing with the action that resolves it. A click selects one photo, Ctrl-click adds or removes one,
// Shift-click selects a range and Ctrl+A selects all; a double-click or Space opens the viewer on the
// last photo clicked, the arrow keys move. The header's Save and Delete and the keys act on the
// selection, a photo's menu on the selection when the photo is part of it.
Rectangle {
    id: column

    // On screen: the section shows in a window that is not hidden.
    readonly property bool shown: visible && Window.window !== null && Window.window.visible
    property var photosState: null
    property var ui: ({ selected: [], focus: null, viewer: null, busy: [], requestPending: false })
    // The gallery's photos by id, and their ids in gallery order.
    property var photosById: ({})
    property var order: []
    readonly property var selectedSet: setOf(ui.selected)
    readonly property var busySet: setOf(ui.busy)
    readonly property string content: photosState ? photosState.content : "nothing"
    readonly property string phoneName: EngineBridge.phoneName.length > 0 ? EngineBridge.phoneName : L10n.t("your phone")
    readonly property string subtitle: photosState && photosState.syncing ? L10n.t("Syncing...")
        : L10n.f("Recent photos on %@", [phoneName])
    readonly property var selectedInOrder: order.filter(id => selectedSet[id] === true)
    readonly property var deletableSelection: selectedInOrder.filter(id => canDelete(photosById[id]))
    // The focus the grid last scrolled to.
    property var shownFocus: null

    function setOf(ids) {
        const set = {}
        ids.forEach(id => { set[id] = true })
        return set
    }

    function canDelete(photo) {
        return photo !== undefined && photosState !== null && photosState.deleteAvailable === true
            && (photo.hasThumbnail || photo.hasFull) && photo.deleteState === "none"
    }

    function sync() {
        if (!shown)
            return
        const state = JSON.parse(EngineBridge.query("photos.state", ""))
        // Only the gallery state shows the grid, so only then do its cells ask for thumbnails.
        const list = state !== null && state.content === "gallery" ? JSON.parse(EngineBridge.query("photos", "")) || [] : []
        const byId = {}
        list.forEach(photo => { byId[photo.id] = photo })
        photosState = state
        photosById = byId
        order = list.map(photo => photo.id)
        photoModel.sync(list, photo => photo.id)
        layoutTimer.restart()
    }

    function perform(name, value) {
        EngineBridge.perform(name, typeof value === "string" ? value : JSON.stringify(value))
    }

    function reportVisible() {
        if (!shown || photoModel.count === 0)
            return
        const first = Math.max(0, grid.indexAt(4, grid.contentY + 4))
        let last = grid.indexAt(grid.width - 8, grid.contentY + grid.height - 4)
        if (last < 0)
            last = photoModel.count - 1
        const ids = []
        for (let index = first; index <= last; index++)
            ids.push(Number(photoModel.get(index).key))
        perform("photos.layout", ids)
    }

    // A click as a file manager reads it: Ctrl adds or removes the photo, Shift extends the range, a
    // plain click selects the photo alone.
    function click(id, modifiers) {
        grid.forceActiveFocus()
        const mode = modifiers & Qt.ControlModifier ? "toggle" : modifiers & Qt.ShiftModifier ? "extend" : "only"
        perform("photos.select", { id: id, mode: mode })
    }

    // The menu of photo `id`, at (x, y) in its tile `item`.
    function openMenu(id, item, x, y) {
        grid.forceActiveFocus()
        perform("photos.contextMenu", String(id))
        const info = JSON.parse(EngineBridge.query("photos.menu", String(id)))
        if (info === null)
            return
        photoMenu.photoId = id
        photoMenu.targets = info.targets
        photoMenu.deletable = info.deletable
        photoMenu.apps = info.openWith
        // Open With is offered for one photo only.
        for (let index = 0; index < photoMenu.count; index++) {
            const row = photoMenu.itemAt(index)
            if (row && row.subMenu === openWithMenu)
                row.visible = info.targets.length === 1
        }
        if (item)
            photoMenu.popup(item, x, y)
        else
            photoMenu.popup()
    }

    function save(ids) {
        if (ids.length === 1)
            perform("photos.startSave", String(ids[0]))
        else if (ids.length > 1)
            folderDialog.ask(ids)
    }

    function startDrag(tile) {
        const url = JSON.parse(EngineBridge.query("photos.dragFile", String(tile.photoId)))
        if (url === null) {
            // The full image is not here yet: it is fetched, and the photo can be dragged out once it is.
            perform("photos.prepareDrag", String(tile.photoId))
            return
        }
        dragSource.Drag.mimeData = { "text/uri-list": url + "\r\n" }
        dragSource.Drag.active = true
        dragSource.Drag.startDrag(Qt.CopyAction)
        dragSource.Drag.active = false
    }

    function tileOf(id) {
        const index = order.indexOf(id)
        return index < 0 ? null : grid.itemAtIndex(index)
    }

    color: Theme.windowBackground

    onShownChanged: {
        perform(shown ? "photos.entered" : "photos.left", "")
        sync()
    }
    Component.onCompleted: {
        ui = JSON.parse(EngineBridge.query("photos.ui", "")) || ui
        sync()
    }
    // The grid scrolls to the focus when it moves (not on every change of the selection's state).
    onUiChanged: {
        if (ui.focus === shownFocus)
            return
        shownFocus = ui.focus
        const index = ui.focus === null ? -1 : order.indexOf(ui.focus)
        if (index >= 0)
            grid.positionViewAtIndex(index, GridView.Contain)
    }

    KeyedListModel {
        id: photoModel
    }

    // The gallery changes in bursts while the phone sends thumbnails: it is read again at most every
    // 150 ms.
    Timer {
        id: syncTimer
        interval: 150
        onTriggered: column.sync()
    }

    Connections {
        target: EngineBridge
        function onPhotosRevisionChanged() {
            if (!syncTimer.running)
                syncTimer.start()
        }
        function onAreaEvent(area, name, payload) {
            if (area !== "photos")
                return
            const data = JSON.parse(payload)
            if (name === "ui")
                column.ui = data
            else if (name === "prompt")
                prompt.ask(data)
            else if (name === "saveDialog")
                saveDialog.ask(data)
        }
    }

    Timer {
        id: layoutTimer
        interval: 250
        onTriggered: column.reportVisible()
    }

    Item {
        id: dragSource
        Drag.dragType: Drag.None
        Drag.supportedActions: Qt.CopyAction
    }

    ColumnLayout {
        anchors.fill: parent
        spacing: 0

        RowLayout {
            Layout.fillWidth: true
            Layout.preferredHeight: 52
            Layout.leftMargin: 20
            Layout.rightMargin: 14
            spacing: 8

            ColumnLayout {
                Layout.fillWidth: true
                spacing: 0

                Text {
                    Layout.fillWidth: true
                    text: L10n.t("Photos")
                    color: Theme.text
                    font.pixelSize: Theme.headlineSize
                    font.weight: Font.Bold
                }

                Text {
                    Layout.fillWidth: true
                    visible: column.content === "gallery" || column.content === "empty"
                    text: column.content === "gallery" && column.ui.selected.length > 0
                        ? L10n.f("%lld Selected", [column.ui.selected.length]) : column.subtitle
                    color: Theme.secondaryText
                    font.pixelSize: Theme.subheadSize
                    elide: Text.ElideRight
                }
            }

            IconButton {
                id: saveButton
                visible: column.content === "gallery"
                iconName: "save"
                label: L10n.t("Save the selected photos")
                onClicked: saveMenu.popup(saveButton, 0, saveButton.height + 4)
            }

            IconButton {
                visible: column.content === "gallery" && column.photosState.deleteAvailable === true
                enabled: column.deletableSelection.length > 0
                iconName: "trash"
                label: L10n.t("Delete from Phone")
                onClicked: column.perform("photos.delete", column.deletableSelection)
            }
        }

        // The phone shares only the photos the user picked: one line above the grid, until dismissed.
        Rectangle {
            Layout.fillWidth: true
            Layout.leftMargin: 16
            Layout.rightMargin: 14
            Layout.bottomMargin: 8
            visible: column.content === "gallery" && column.photosState.fullAccessReminder === true
            implicitHeight: noticeRow.implicitHeight + 16
            radius: Theme.moduleRadius
            color: Theme.listBackground
            border.color: Theme.separator

            RowLayout {
                id: noticeRow
                anchors.fill: parent
                anchors.margins: 8
                anchors.leftMargin: 12
                spacing: 8

                Text {
                    Layout.fillWidth: true
                    text: L10n.t("Phone Link can see only some of your photos.")
                    color: Theme.secondaryText
                    font.pixelSize: Theme.subheadSize
                    wrapMode: Text.Wrap
                }

                PillButton {
                    text: L10n.t("Not Now")
                    onClicked: column.perform("photos.dismissReminder", "")
                }

                PillButton {
                    text: L10n.t("Allow Full Access")
                    enabled: !column.ui.requestPending
                    onClicked: column.perform("photos.requestFullAccess", "")
                }
            }
        }

        Item {
            Layout.fillWidth: true
            Layout.fillHeight: true

            GridView {
                id: grid

                readonly property int columns: Math.max(1, Math.ceil((width + 2) / 182))

                anchors.fill: parent
                anchors.leftMargin: 16
                anchors.rightMargin: 8
                visible: column.content === "gallery"
                clip: true
                focus: true
                activeFocusOnTab: true
                model: photoModel
                cellWidth: width / columns
                cellHeight: cellWidth
                ScrollBar.vertical: ScrollBar {}
                onContentYChanged: layoutTimer.restart()
                onHeightChanged: layoutTimer.restart()
                Accessible.role: Accessible.List
                Accessible.name: L10n.t("Photos")

                Keys.onPressed: event => {
                    const control = (event.modifiers & Qt.ControlModifier) !== 0
                    switch (event.key) {
                    case Qt.Key_Left: column.perform("photos.move", "-1"); break
                    case Qt.Key_Right: column.perform("photos.move", "1"); break
                    case Qt.Key_Up: column.perform("photos.move", String(-grid.columns)); break
                    case Qt.Key_Down: column.perform("photos.move", String(grid.columns)); break
                    case Qt.Key_Space: column.perform("photos.toggleViewer", ""); break
                    case Qt.Key_Delete:
                    case Qt.Key_Backspace:
                        if (column.deletableSelection.length > 0)
                            column.perform("photos.delete", column.deletableSelection)
                        break
                    case Qt.Key_Menu:
                        if (column.ui.focus !== null) {
                            const tile = column.tileOf(column.ui.focus)
                            column.openMenu(column.ui.focus, tile, tile ? tile.width / 2 : 0, tile ? tile.height / 2 : 0)
                        }
                        break
                    case Qt.Key_A:
                        if (!control)
                            return
                        column.perform("photos.selectAll", "")
                        break
                    case Qt.Key_C:
                        if (!control)
                            return
                        if (column.selectedInOrder.length > 0)
                            column.perform("photos.copy", column.selectedInOrder)
                        break
                    default:
                        return
                    }
                    event.accepted = true
                }

                delegate: PhotoTile {
                    id: tile
                    width: grid.cellWidth
                    height: grid.cellHeight
                    selected: column.selectedSet[photoId] === true
                    busy: column.busySet[photoId] === true
                    onTapped: modifiers => column.click(photoId, modifiers)
                    onDoubleTapped: {
                        grid.forceActiveFocus()
                        column.perform("photos.openViewer", String(photoId))
                    }
                    onMenuRequested: (x, y) => column.openMenu(photoId, tile, x, y)
                    onDragRequested: column.startDrag(tile)
                }
            }

            ColumnLayout {
                anchors.centerIn: parent
                width: Math.min(parent.width - 40, 440)
                visible: column.content !== "gallery" && column.content !== "nothing"
                spacing: 16

                EmptyState {
                    Layout.fillWidth: true
                    iconName: "photo"
                    title: {
                        switch (column.content) {
                        case "unavailable": return L10n.t("Photos Aren't Available")
                        case "turnedOff": return L10n.t("Photos Are Off on This Computer")
                        case "needsPhoneAccess": return L10n.t("Allow Photo Access on Your Phone")
                        default: return L10n.t("No Photos")
                        }
                    }
                    description: {
                        switch (column.content) {
                        case "unavailable":
                            return L10n.f("Link to Windows on %@ doesn't share photos with this computer.", [column.phoneName])
                        case "turnedOff":
                            return L10n.f("Turn on Photos to see recent photos from %@.", [column.phoneName])
                        case "needsPhoneAccess":
                            return L10n.f("Recent photos from %@ appear here after you allow access on the phone.", [column.phoneName])
                        default:
                            return column.photosState && column.photosState.fullAccessReminder === true
                                ? L10n.t("Phone Link can see only some of your photos.")
                                : L10n.f("Recent photos from %@ appear here.", [column.phoneName])
                        }
                    }
                }

                PillButton {
                    Layout.alignment: Qt.AlignHCenter
                    visible: column.content === "turnedOff"
                    prominent: true
                    text: L10n.t("Turn On")
                    onClicked: column.perform("photos.turnOn", "")
                }

                PillButton {
                    Layout.alignment: Qt.AlignHCenter
                    visible: column.content === "needsPhoneAccess"
                    prominent: true
                    enabled: !column.ui.requestPending
                    text: L10n.t("Send Request to Phone")
                    onClicked: column.perform("photos.requestAccess", "")
                }

                PillButton {
                    Layout.alignment: Qt.AlignHCenter
                    visible: column.content === "empty" && column.photosState.fullAccessReminder === true
                    prominent: true
                    enabled: !column.ui.requestPending
                    text: L10n.t("Allow Full Access")
                    onClicked: column.perform("photos.requestFullAccess", "")
                }
            }
        }
    }

    PhotoViewer {
        id: viewer
        anchors.fill: parent
        photoId: column.ui.viewer === null ? -1 : column.ui.viewer
        columns: grid.columns
        onMoveRequested: offset => column.perform("photos.move", String(offset))
        onCloseRequested: {
            column.perform("photos.closeViewer", "")
            grid.forceActiveFocus()
        }
        onOpenRequested: column.perform("photos.open", [photoId])
        onCopyRequested: column.perform("photos.copy", [photoId])
        onSaveRequested: column.save([photoId])
    }

    AppsMenu {
        id: saveMenu

        AppsMenuItem {
            text: column.selectedInOrder.length > 1 ? L10n.p("Save %lld Photos...", column.selectedInOrder.length, [column.selectedInOrder.length])
                : L10n.t("Save Photo...")
            enabled: column.selectedInOrder.length > 0
            onTriggered: column.save(column.selectedInOrder)
        }

        AppsMenuItem {
            text: L10n.t("Save All Photos...")
            enabled: column.order.length > 0
            onTriggered: folderDialog.ask(column.order)
        }
    }

    // A photo's menu: most used first, Delete last and only when something can be deleted. It acts on
    // the selection when the photo is part of it, otherwise on the photo alone.
    AppsMenu {
        id: photoMenu

        property double photoId: -1
        property var targets: []
        property var deletable: []
        property var apps: []
        readonly property bool single: targets.length === 1

        AppsMenuItem {
            text: photoMenu.single ? L10n.t("Open") : L10n.p("Open %lld Photos", photoMenu.targets.length, [photoMenu.targets.length])
            onTriggered: column.perform("photos.open", photoMenu.targets)
        }

        AppsMenu {
            id: openWithMenu
            title: L10n.t("Open With")

            Instantiator {
                model: photoMenu.apps
                delegate: AppsMenuItem {
                    required property var modelData
                    text: modelData.isDefault ? L10n.f("%@ (default)", [modelData.name]) : modelData.name
                    onTriggered: column.perform("photos.openWith", { id: photoMenu.photoId, app: modelData.id })
                }
                onObjectAdded: (index, object) => openWithMenu.insertItem(index, object)
                onObjectRemoved: (index, object) => openWithMenu.removeItem(object)
            }

            AppsMenuSeparator {
                visible: photoMenu.apps.length > 0
            }

            AppsMenuItem {
                text: L10n.t("Other...")
                onTriggered: column.perform("photos.openWithOther", String(photoMenu.photoId))
            }
        }

        AppsMenuSeparator {}

        AppsMenuItem {
            text: photoMenu.single ? L10n.t("Copy") : L10n.p("Copy %lld Photos", photoMenu.targets.length, [photoMenu.targets.length])
            onTriggered: column.perform("photos.copy", photoMenu.targets)
        }

        AppsMenuItem {
            text: photoMenu.single ? L10n.t("Save As...") : L10n.p("Save %lld Photos...", photoMenu.targets.length, [photoMenu.targets.length])
            onTriggered: column.save(photoMenu.targets)
        }

        AppsMenuSeparator {
            visible: photoMenu.deletable.length > 0
        }

        AppsMenuItem {
            visible: photoMenu.deletable.length > 0
            destructive: true
            text: photoMenu.single ? L10n.t("Delete...") : L10n.p("Delete %lld Photos...", photoMenu.deletable.length, [photoMenu.deletable.length])
            onTriggered: column.perform("photos.delete", photoMenu.deletable)
        }
    }

    // Deleting on the phone can't be undone, so the user confirms it; or the phone has not allowed
    // deleting, and the question offers to ask it.
    MessageDialog {
        id: prompt

        property var current: ({ kind: "", ids: [], name: "" })
        readonly property bool confirming: current.kind === "confirmDelete"
        readonly property int count: confirming ? current.ids.length : 0

        function ask(data) {
            current = data
            open()
        }

        heading: !confirming ? L10n.t("Allow Deleting Photos on Your Phone?")
            : count > 1 ? L10n.p("Delete %lld Photos from Your Phone?", count, [count]) : L10n.t("Delete This Photo from Your Phone?")
        message: !confirming ? L10n.t("Your phone needs to allow Phone Link to delete photos.")
            : count > 1 ? L10n.p("These %lld photos will be deleted from your phone. This can't be undone.", count, [count])
            : current.name.length === 0 ? L10n.t("This can't be undone.")
            : L10n.f("“%@” will be deleted from your phone. This can't be undone.", [current.name])
        acceptText: confirming ? L10n.t("Delete") : L10n.t("Allow on Phone")
        destructive: confirming
        onAccepted: {
            if (confirming)
                column.perform("photos.confirmDelete", current.ids)
            else
                column.perform("photos.allowDeleting", "")
        }
    }

    // Save As…: the photo's full image is on its way while the user picks the file.
    FileDialog {
        id: saveDialog

        property double photoId: -1

        function ask(data) {
            photoId = data.id
            selectedFile = StandardPaths.writableLocation(StandardPaths.PicturesLocation) + "/" + encodeURIComponent(data.name)
            open()
        }

        fileMode: FileDialog.SaveFile
        onAccepted: column.perform("photos.saveAs", { id: photoId, path: selectedFile.toString() })
        onRejected: column.perform("photos.cancelSave", String(photoId))
    }

    // Save N Photos… and Save All Photos…: the folder is chosen once, then each photo is saved there.
    FolderDialog {
        id: folderDialog

        property var ids: []

        function ask(list) {
            ids = list
            open()
        }

        title: L10n.p("Choose a folder to save %lld photos in.", ids.length, [ids.length])
        acceptLabel: L10n.t("Save")
        currentFolder: StandardPaths.writableLocation(StandardPaths.PicturesLocation)
        onAccepted: column.perform("photos.saveToFolder", { ids: ids, folder: selectedFolder.toString() })
    }
}
