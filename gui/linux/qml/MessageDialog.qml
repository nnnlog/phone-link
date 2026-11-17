import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import PhoneLink

// A question over the window: a heading, a message, Cancel and the action. Escape cancels.
Dialog {
    id: dialog

    property string heading
    property string message
    property string acceptText
    property bool destructive: false

    parent: Overlay.overlay
    anchors.centerIn: parent
    width: Math.min(400, parent ? parent.width - 48 : 400)
    modal: true
    focus: true
    padding: 20

    background: Rectangle {
        radius: Theme.moduleRadius
        color: Theme.raisedBackground
        border.color: Theme.separator
    }

    contentItem: ColumnLayout {
        spacing: 10

        Text {
            Layout.fillWidth: true
            text: dialog.heading
            color: Theme.text
            font.pixelSize: Theme.headlineSize - 2
            font.weight: Font.DemiBold
            wrapMode: Text.Wrap
        }

        Text {
            Layout.fillWidth: true
            visible: text.length > 0
            text: dialog.message
            color: Theme.secondaryText
            font.pixelSize: Theme.bodySize
            wrapMode: Text.Wrap
        }

        RowLayout {
            Layout.alignment: Qt.AlignRight
            Layout.topMargin: 8
            spacing: 8

            PillButton {
                text: L10n.t("Cancel")
                onClicked: dialog.reject()
            }

            PillButton {
                prominent: true
                destructive: dialog.destructive
                text: dialog.acceptText
                onClicked: dialog.accept()
            }
        }
    }
}
