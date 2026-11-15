import QtQuick
import QtQuick.Controls.Basic

TextField {
    id: field

    property bool submittable: true

    signal submitted(string text)

    leftPadding: 12
    rightPadding: 36
    topPadding: 6
    bottomPadding: 6
    font.pixelSize: Theme.bodySize
    color: Theme.text
    placeholderTextColor: Theme.secondaryText
    selectionColor: Theme.accent
    selectedTextColor: "white"

    function submit() {
        if (!field.submittable || field.text.length === 0)
            return
        field.submitted(field.text)
        field.text = ""
    }

    onAccepted: submit()

    background: Rectangle {
        implicitHeight: 30
        radius: height / 2
        color: Theme.fieldBackground
        border.color: field.activeFocus ? Theme.accent : Theme.fieldBorder
        border.width: 1
    }

    AbstractButton {
        anchors.right: parent.right
        anchors.rightMargin: 4
        anchors.verticalCenter: parent.verticalCenter
        width: 22
        height: 22
        visible: field.submittable && field.text.length > 0
        onClicked: field.submit()

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
