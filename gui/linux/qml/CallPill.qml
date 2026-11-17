import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import PhoneLink

// An action of a call or contact card: an icon and its text in a pill, or the icon alone. Its help
// shows as a tooltip also while the action is unavailable, saying why.
Item {
    id: pill

    property string text
    property string iconName
    property bool iconOnly: false
    property string help
    // Whether the action can be taken now.
    property bool available: true

    signal clicked()

    implicitWidth: button.implicitWidth
    implicitHeight: button.implicitHeight

    ToolTip.visible: hover.hovered && help.length > 0
    ToolTip.delay: 600
    ToolTip.text: help

    HoverHandler {
        id: hover
    }

    AbstractButton {
        id: button

        anchors.fill: parent
        enabled: pill.available
        text: pill.text
        hoverEnabled: true
        focusPolicy: Qt.StrongFocus
        implicitWidth: pill.iconOnly ? 28 : content.implicitWidth + 28
        implicitHeight: pill.iconOnly ? 28 : 30

        Accessible.role: Accessible.Button
        Accessible.name: pill.text
        Accessible.description: pill.help
        onClicked: pill.clicked()

        background: Rectangle {
            radius: height / 2
            color: button.hovered ? Theme.controlHover : pill.iconOnly ? "transparent" : Theme.controlBackground
            border.color: button.visualFocus ? Theme.accent : "transparent"
            opacity: button.down ? 0.75 : 1
        }

        contentItem: Item {
            RowLayout {
                id: content

                anchors.centerIn: parent
                spacing: 6

                Icon {
                    name: pill.iconName
                    size: pill.iconOnly ? 16 : 14
                    color: pill.iconOnly ? Theme.accent : Theme.text
                    opacity: button.enabled ? 1 : 0.4
                }

                Text {
                    visible: !pill.iconOnly
                    text: pill.text
                    color: Theme.text
                    font.pixelSize: Theme.bodySize
                    opacity: button.enabled ? 1 : 0.4
                }
            }
        }
    }
}
