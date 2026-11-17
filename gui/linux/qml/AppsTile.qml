import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import PhoneLink

// An app's tile: its icon over its name, the app's notification count, and for a recent task how it
// carries over to this computer. A click (or Return) opens it, a right-click (or the Menu key) opens its
// menu; a draggable tile drags onto the favorites, or back onto every app.
Item {
    id: tile

    // {package, name, hasIcon, count, stamp}
    property var app: ({})
    property bool draggable: false
    // An app dropped on this tile (a favorite) moves here.
    property bool dropTarget: false

    signal activated()
    // A right-click at (x, y) in the tile, or the Menu key.
    signal menuRequested(real x, real y)
    signal appDropped(string packageName)

    width: 96
    height: 106
    activeFocusOnTab: true

    Accessible.role: Accessible.Button
    Accessible.name: app.count > 0 ? L10n.p("%@, %lld notifications", app.count, [app.name, app.count]) : app.name || ""
    Accessible.onPressAction: activated()

    Keys.onPressed: event => {
        if (event.key === Qt.Key_Return || event.key === Qt.Key_Enter || event.key === Qt.Key_Space)
            tile.activated()
        else if (event.key === Qt.Key_Menu || (event.key === Qt.Key_F10 && (event.modifiers & Qt.ShiftModifier)))
            tile.menuRequested(tile.width / 2, tile.height / 2)
        else
            return
        event.accepted = true
    }

    Drag.dragType: Drag.None
    Drag.supportedActions: Qt.MoveAction
    Drag.keys: ["application/x-phonelink-app"]
    Drag.imageSource: glyph.iconUrl

    Rectangle {
        anchors.fill: parent
        anchors.margins: 2
        radius: Theme.rowRadius
        color: mouse.containsMouse || drop.containsDrag ? Theme.hover : "transparent"
        border.color: tile.activeFocus ? Theme.accent : drop.containsDrag ? Theme.separator : "transparent"
    }

    ColumnLayout {
        anchors.top: parent.top
        anchors.topMargin: 10
        anchors.horizontalCenter: parent.horizontalCenter
        width: parent.width - 10
        spacing: 6

        Item {
            Layout.alignment: Qt.AlignHCenter
            implicitWidth: 48
            implicitHeight: 48

            AppGlyph {
                id: glyph
                anchors.fill: parent
                name: tile.app.name || ""
                packageName: tile.app.hasIcon ? tile.app["package"] : ""
                size: 48
            }

            Rectangle {
                anchors.right: parent.right
                anchors.top: parent.top
                anchors.rightMargin: -6
                anchors.topMargin: -6
                visible: tile.app.count > 0
                width: Math.max(18, countText.implicitWidth + 10)
                height: 18
                radius: 9
                color: Theme.destructive

                Text {
                    id: countText
                    anchors.centerIn: parent
                    text: tile.app.count > 0 ? Number(tile.app.count).toLocaleString(Qt.locale(), "f", 0) : ""
                    color: "white"
                    font.pixelSize: Theme.captionSize - 1
                    font.weight: Font.Bold
                }
            }

            Rectangle {
                anchors.right: parent.right
                anchors.bottom: parent.bottom
                anchors.rightMargin: -6
                anchors.bottomMargin: -6
                visible: !!tile.app.stamp
                width: 20
                height: 20
                radius: 10
                color: Theme.raisedBackground
                border.color: Theme.separator

                Icon {
                    anchors.centerIn: parent
                    name: tile.app.stamp === "desktop" ? "computer" : tile.app.stamp === "browser" ? "globe" : "arrowUpForward"
                    size: 12
                    color: Theme.secondaryText
                }
            }
        }

        Text {
            Layout.fillWidth: true
            text: tile.app.name || ""
            color: Theme.text
            font.pixelSize: Theme.captionSize + 1
            horizontalAlignment: Text.AlignHCenter
            wrapMode: Text.Wrap
            maximumLineCount: 2
            elide: Text.ElideRight
        }
    }

    MouseArea {
        id: mouse

        property point pressedAt
        property bool dragged: false

        anchors.fill: parent
        acceptedButtons: Qt.LeftButton | Qt.RightButton
        hoverEnabled: true
        ToolTip.visible: containsMouse && !pressed
        ToolTip.delay: 800
        ToolTip.text: tile.app.name || ""
        onPressed: event => {
            pressedAt = Qt.point(event.x, event.y)
            dragged = false
            if (event.button === Qt.RightButton)
                tile.menuRequested(event.x, event.y)
        }
        onClicked: event => {
            if (event.button === Qt.LeftButton && !dragged)
                tile.activated()
        }
        onPositionChanged: event => {
            if (!tile.draggable || dragged || !(event.buttons & Qt.LeftButton))
                return
            if (Math.abs(event.x - pressedAt.x) + Math.abs(event.y - pressedAt.y) < Qt.styleHints.startDragDistance)
                return
            dragged = true
            tile.Drag.mimeData = { "application/x-phonelink-app": tile.app["package"] }
            tile.Drag.active = true
            tile.Drag.startDrag(Qt.MoveAction)
            tile.Drag.active = false
        }
    }

    DropArea {
        id: drop
        anchors.fill: parent
        enabled: tile.dropTarget
        keys: ["application/x-phonelink-app"]
        onDropped: event => {
            const dropped = event.getDataAsString("application/x-phonelink-app")
            if (dropped.length === 0 || dropped === tile.app["package"])
                return
            tile.appDropped(dropped)
            event.accept(Qt.MoveAction)
        }
    }
}
