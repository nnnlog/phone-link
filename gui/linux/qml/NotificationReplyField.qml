import QtQuick
import QtQuick.Controls.Basic
import PhoneLink

// A notification's reply field: Return or the send button asks to send the text. The field keeps the
// text until the reply is queued (the detail clears it then), so a reply that could not be queued is
// not lost; blank text is never sent.
CapsuleField {
    id: field

    readonly property bool canSend: text.trim().length > 0

    signal sendRequested(string text)

    submittable: false
    onAccepted: {
        if (field.canSend)
            field.sendRequested(field.text)
    }

    AbstractButton {
        anchors.right: parent.right
        anchors.rightMargin: 4
        anchors.verticalCenter: parent.verticalCenter
        width: 22
        height: 22
        visible: field.canSend
        hoverEnabled: true
        focusPolicy: Qt.NoFocus
        Accessible.role: Accessible.Button
        Accessible.name: L10n.t("Send")
        ToolTip.visible: hovered
        ToolTip.delay: 600
        ToolTip.text: L10n.t("Send")
        onClicked: field.sendRequested(field.text)

        background: Rectangle {
            radius: width / 2
            color: Theme.accent
        }

        contentItem: Item {
            Icon {
                anchors.centerIn: parent
                name: "send"
                size: 14
                color: "white"
            }
        }
    }
}
