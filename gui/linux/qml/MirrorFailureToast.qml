import QtQuick
import QtQuick.Layouts
import PhoneLink

// A failed action shown in a mirroring or camera window while it is the active window, as the main
// window shows it: for a few seconds (longer while the pointer is on it), or until dismissed.
Rectangle {
    id: toast

    property var queue: []
    readonly property var current: queue.length > 0 ? queue[0] : null

    function show(title, message) {
        queue = queue.concat([{ title: title, message: message }])
    }

    function dismiss() {
        queue = queue.slice(1)
    }

    width: Math.min(440, parent.width - 24)
    implicitHeight: toastRow.implicitHeight + 24
    z: 20
    visible: current !== null
    radius: Theme.moduleRadius
    color: Theme.raisedBackground
    border.color: Theme.separator
    Accessible.role: Accessible.AlertMessage
    Accessible.name: current ? current.title + ". " + current.message : ""
    onCurrentChanged: {
        if (current !== null)
            toastTimer.restart()
    }

    Timer {
        id: toastTimer
        interval: 6000
        running: toast.current !== null && !toastHover.hovered
        onTriggered: toast.dismiss()
    }

    HoverHandler {
        id: toastHover
    }

    TapHandler {
        onTapped: toast.dismiss()
    }

    RowLayout {
        id: toastRow
        anchors.fill: parent
        anchors.margins: 12
        spacing: 10

        Icon {
            Layout.alignment: Qt.AlignTop
            Layout.topMargin: 2
            name: "warning"
            size: 18
            color: Theme.warning
        }

        ColumnLayout {
            Layout.fillWidth: true
            spacing: 2

            Text {
                Layout.fillWidth: true
                text: toast.current ? toast.current.title : ""
                color: Theme.text
                font.pixelSize: Theme.bodySize
                font.weight: Font.DemiBold
                wrapMode: Text.Wrap
            }

            Text {
                Layout.fillWidth: true
                visible: text.length > 0
                text: toast.current ? toast.current.message : ""
                color: Theme.secondaryText
                font.pixelSize: Theme.subheadSize
                wrapMode: Text.Wrap
            }
        }

        IconButton {
            Layout.alignment: Qt.AlignTop
            implicitWidth: 22
            implicitHeight: 22
            focusPolicy: Qt.NoFocus
            iconName: "close"
            label: L10n.t("Dismiss")
            onClicked: toast.dismiss()
        }
    }
}
