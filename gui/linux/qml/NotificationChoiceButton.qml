import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import PhoneLink

// An app's pop-up in the Notifications settings pane: how its notifications reach this computer —
// as desktop notifications and in the list, in the list only, or not at all.
AbstractButton {
    id: button

    // "bannersAndList", "listOnly" or "off".
    property string choice
    property string appName

    signal chosen(string choice)

    readonly property var choices: [
        { key: "bannersAndList", title: L10n.t("Banners and List") },
        { key: "listOnly", title: L10n.t("List Only") },
        { key: "off", title: L10n.t("Off") }
    ]
    readonly property string title: (choices.find(c => c.key === choice) || { title: "" }).title

    function openMenu() {
        menu.show(choices.map(c => ({ text: c.title, chosen: c.key === button.choice, onTriggered: () => button.chosen(c.key) })),
                  button, 0, button.height + 4)
    }

    // As wide as the widest choice, so the pop-ups of a list line up.
    implicitWidth: Math.max(widths[0].advanceWidth, widths[1].advanceWidth, widths[2].advanceWidth) + chevron.width + 30
    implicitHeight: 28

    readonly property list<TextMetrics> widths: [
        TextMetrics { text: button.choices[0].title; font.pixelSize: Theme.bodySize },
        TextMetrics { text: button.choices[1].title; font.pixelSize: Theme.bodySize },
        TextMetrics { text: button.choices[2].title; font.pixelSize: Theme.bodySize }
    ]
    hoverEnabled: true
    Accessible.role: Accessible.ComboBox
    Accessible.name: appName
    Accessible.description: title
    onClicked: openMenu()
    Keys.onReturnPressed: openMenu()
    Keys.onDownPressed: openMenu()

    background: Rectangle {
        radius: height / 2
        color: button.hovered ? Theme.controlHover : Theme.controlBackground
        border.color: button.visualFocus ? Theme.accent : "transparent"
        opacity: button.down ? 0.75 : 1
    }

    contentItem: Item {
        Text {
            anchors.left: parent.left
            anchors.leftMargin: 12
            anchors.verticalCenter: parent.verticalCenter
            text: button.title
            color: Theme.text
            font.pixelSize: Theme.bodySize
            opacity: button.enabled ? 1 : 0.4
        }

        Icon {
            id: chevron
            anchors.right: parent.right
            anchors.rightMargin: 10
            anchors.verticalCenter: parent.verticalCenter
            name: "chevronDown"
            size: 12
            color: Theme.secondaryText
            opacity: button.enabled ? 1 : 0.4
        }
    }

    NotificationMenu {
        id: menu
    }
}
