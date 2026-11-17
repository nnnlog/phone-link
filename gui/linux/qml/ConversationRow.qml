import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import PhoneLink

// A row of the conversation list: an unread dot, a picture, the title with its time and muted mark,
// and the line under it. A right-click, the Menu key or Shift+F10 asks for the row's menu.
AbstractButton {
    id: row

    property string title
    property string preview
    property string time
    property bool unread: false
    property bool muted: false
    property bool selected: false
    // The picture before the text (40 px).
    property alias leading: picture.data

    // At (x, y) in the row.
    signal menuRequested(real x, real y)

    implicitHeight: 64
    hoverEnabled: true
    focusPolicy: Qt.StrongFocus
    Accessible.role: Accessible.Button
    Accessible.name: unread ? title + ", " + L10n.t("Unread") : title
    Accessible.description: preview
    Keys.onMenuPressed: row.menuRequested(12, row.height)
    Keys.onPressed: event => {
        if (event.key === Qt.Key_F10 && (event.modifiers & Qt.ShiftModifier)) {
            row.menuRequested(12, row.height)
            event.accepted = true
        }
    }

    TapHandler {
        acceptedButtons: Qt.RightButton
        onTapped: eventPoint => row.menuRequested(eventPoint.position.x, eventPoint.position.y)
    }

    background: Rectangle {
        radius: Theme.rowRadius
        color: row.selected ? Theme.selection : row.hovered ? Theme.hover : "transparent"
        border.width: row.visualFocus ? 2 : 0
        border.color: Theme.accent
    }

    contentItem: RowLayout {
        spacing: 8

        Rectangle {
            Layout.preferredWidth: 7
            Layout.preferredHeight: 7
            Layout.leftMargin: 2
            radius: 3.5
            color: row.unread ? Theme.accent : "transparent"
        }

        Item {
            id: picture
            Layout.preferredWidth: 40
            Layout.preferredHeight: 40
        }

        ColumnLayout {
            Layout.fillWidth: true
            Layout.rightMargin: 6
            spacing: 1

            RowLayout {
                Layout.fillWidth: true
                spacing: 4

                Text {
                    Layout.fillWidth: true
                    text: row.title
                    color: Theme.text
                    font.pixelSize: Theme.bodySize + 1
                    font.weight: Font.Bold
                    elide: Text.ElideRight
                }

                Icon {
                    visible: row.muted
                    name: "bellSlash"
                    size: 12
                    color: Theme.secondaryText
                    Accessible.role: Accessible.Graphic
                    Accessible.name: L10n.t("Alerts Hidden")
                }

                Text {
                    visible: text.length > 0
                    text: row.time
                    color: Theme.secondaryText
                    font.pixelSize: Theme.captionSize
                }
            }

            Text {
                Layout.fillWidth: true
                visible: text.length > 0
                text: row.preview
                color: Theme.secondaryText
                font.pixelSize: Theme.subheadSize
                wrapMode: Text.Wrap
                maximumLineCount: 2
                elide: Text.ElideRight
            }
        }
    }
}
