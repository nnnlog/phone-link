import QtQuick
import QtQuick.Controls.Basic

Switch {
    id: control

    indicator: Rectangle {
        implicitWidth: 38
        implicitHeight: 22
        x: control.width - width - control.rightPadding
        y: (control.height - height) / 2
        radius: height / 2
        color: control.checked ? Theme.accent : Theme.controlBackground
        border.color: control.checked ? Theme.accent : Theme.fieldBorder
        opacity: control.enabled ? 1 : 0.4

        Rectangle {
            x: control.checked ? parent.width - width - 2 : 2
            y: 2
            width: parent.height - 4
            height: width
            radius: width / 2
            color: "white"

            Behavior on x {
                NumberAnimation { duration: 120 }
            }
        }
    }

    contentItem: Text {
        rightPadding: control.indicator.width + 10
        text: control.text
        color: Theme.text
        font.pixelSize: Theme.bodySize
        verticalAlignment: Text.AlignVCenter
        wrapMode: Text.Wrap
        opacity: control.enabled ? 1 : 0.4
    }
}
