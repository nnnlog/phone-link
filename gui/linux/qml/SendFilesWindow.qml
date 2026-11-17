import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Dialogs
import QtQuick.Layouts
import QtQuick.Shapes
import QtQuick.Window
import PhoneLink

// The Send Files window: drop files on it or choose them, and they go to the phone; it shows the send's
// progress, Cancel and how it ended, and a receive under way. It takes its content's height.
Window {
    id: window

    readonly property var transfers: { EngineBridge.transfersRevision; return JSON.parse(EngineBridge.query("transfers", "")) || {} }
    readonly property string phoneName: EngineBridge.phoneName.length > 0 ? EngineBridge.phoneName : L10n.t("your phone")
    // The query answers nothing before the engine exists: nothing can be sent then.
    readonly property string unavailable: transfers.unavailable === undefined
        ? L10n.t("Phone Link isn't connected to your phone yet.") : transfers.unavailable || ""
    readonly property bool sending: transfers.sending === true
    readonly property var outcome: transfers.outcome || null
    readonly property bool dropZone: unavailable.length === 0 && !sending
    // The green of a success, darker in the light theme so that the outcome's title stays readable.
    readonly property color success: Theme.dark ? "#30d158" : "#248a3d"
    property bool dropping: false

    function send(urls) {
        const files = Array.from(urls, url => url.toString())
        if (files.length > 0)
            EngineBridge.perform("files.sendFromWindow", JSON.stringify(files))
    }

    // A failure of the window's own request, as an alert over it.
    function showFailure(title, message) {
        alert.heading = title
        alert.message = message
        alert.open()
    }

    readonly property int fittedHeight: Math.ceil(column.implicitHeight) + 32

    title: L10n.t("Send Files")
    width: 420
    height: fittedHeight
    minimumWidth: 420
    maximumWidth: 420
    minimumHeight: fittedHeight
    maximumHeight: fittedHeight
    color: Theme.windowBackground
    // A window of its own: it opens from the tray while the main window is hidden.
    transientParent: null

    palette.window: Theme.windowBackground
    palette.windowText: Theme.text
    palette.base: Theme.fieldBackground
    palette.text: Theme.text
    palette.button: Theme.controlBackground
    palette.buttonText: Theme.text
    palette.highlight: Theme.accent
    palette.highlightedText: "white"
    palette.placeholderText: Theme.secondaryText
    palette.light: Theme.paletteLight
    palette.midlight: Theme.paletteMidlight
    palette.mid: Theme.paletteMid
    palette.dark: Theme.paletteDark
    palette.shadow: "black"
    palette.toolTipBase: Theme.toolTipBackground
    palette.toolTipText: Theme.text

    Shortcut {
        sequences: ["Return", "Enter"]
        enabled: window.dropZone && !alert.visible
        onActivated: chooser.open()
    }

    Shortcut {
        sequence: "Escape"
        enabled: window.sending && !alert.visible
        onActivated: EngineBridge.perform("files.cancel", "")
    }

    DropArea {
        anchors.fill: parent
        keys: ["text/uri-list"]
        enabled: window.dropZone
        onEntered: drag => {
            window.dropping = drag.hasUrls
            drag.accepted = drag.hasUrls
        }
        onExited: window.dropping = false
        onDropped: drop => {
            window.dropping = false
            window.send(drop.urls)
            drop.acceptProposedAction()
        }
    }

    ColumnLayout {
        id: column
        x: 16
        y: 16
        width: window.width - 32
        spacing: 12

        // Why the phone cannot be sent files.
        ColumnLayout {
            Layout.fillWidth: true
            Layout.topMargin: 12
            Layout.bottomMargin: 12
            visible: window.unavailable.length > 0
            spacing: 8

            Icon {
                Layout.alignment: Qt.AlignHCenter
                name: "phoneSlash"
                size: 40
                color: Theme.secondaryText
            }

            Text {
                Layout.fillWidth: true
                text: L10n.t("Can't Send Files")
                color: Theme.text
                font.pixelSize: Theme.headlineSize
                font.weight: Font.Bold
                horizontalAlignment: Text.AlignHCenter
                wrapMode: Text.Wrap
            }

            Text {
                Layout.fillWidth: true
                text: window.unavailable
                color: Theme.secondaryText
                font.pixelSize: Theme.bodySize
                horizontalAlignment: Text.AlignHCenter
                wrapMode: Text.Wrap
            }
        }

        // The running send: its files, what it is doing, its progress, and Cancel.
        ColumnLayout {
            Layout.fillWidth: true
            Layout.minimumHeight: 170
            visible: window.unavailable.length === 0 && window.sending
            spacing: 10

            Item {
                Layout.fillHeight: true
            }

            Text {
                Layout.fillWidth: true
                visible: text.length > 0
                text: window.transfers.sendTitle || ""
                color: Theme.text
                font.pixelSize: Theme.bodySize + 1
                font.weight: Font.DemiBold
                elide: Text.ElideMiddle
            }

            ProgressBar {
                id: progress
                Layout.fillWidth: true
                from: 0
                to: 1
                value: window.transfers.sendFraction || 0
                indeterminate: window.transfers.sendFraction === null || window.transfers.sendFraction === undefined
                Accessible.name: window.transfers.sendStatus || ""

                background: Rectangle {
                    implicitHeight: 6
                    radius: 3
                    color: Theme.controlBackground
                }

                contentItem: Item {
                    implicitHeight: 6

                    Rectangle {
                        visible: !progress.indeterminate
                        width: progress.visualPosition * parent.width
                        height: parent.height
                        radius: 3
                        color: Theme.accent
                    }

                    // Indeterminate: a segment that slides across.
                    Rectangle {
                        id: slider
                        visible: progress.indeterminate
                        width: parent.width / 4
                        height: parent.height
                        radius: 3
                        color: Theme.accent

                        SequentialAnimation on x {
                            running: progress.indeterminate && window.visible
                            loops: Animation.Infinite
                            NumberAnimation { from: 0; to: progress.width - slider.width; duration: 900; easing.type: Easing.InOutQuad }
                            NumberAnimation { from: progress.width - slider.width; to: 0; duration: 900; easing.type: Easing.InOutQuad }
                        }
                    }
                }
            }

            RowLayout {
                Layout.fillWidth: true
                spacing: 8

                Text {
                    Layout.fillWidth: true
                    text: window.transfers.sendStatus || ""
                    color: Theme.secondaryText
                    font.pixelSize: Theme.bodySize
                    elide: Text.ElideRight
                }

                PillButton {
                    text: L10n.t("Cancel")
                    onClicked: EngineBridge.perform("files.cancel", "")
                }
            }

            Item {
                Layout.fillHeight: true
            }
        }

        // Where files are dropped; the edge turns the accent color while files are dragged over it.
        Rectangle {
            id: zone
            Layout.fillWidth: true
            Layout.preferredHeight: Math.max(170, dropContent.implicitHeight + 32)
            visible: window.dropZone
            radius: 12
            color: Theme.dark ? Qt.rgba(1, 1, 1, 0.04) : Qt.rgba(0, 0, 0, 0.04)

            Shape {
                anchors.fill: parent
                layer.enabled: true
                layer.samples: 4

                ShapePath {
                    strokeColor: window.dropping ? Theme.accent : Theme.dark ? Qt.rgba(1, 1, 1, 0.16) : Qt.rgba(0, 0, 0, 0.14)
                    strokeWidth: window.dropping ? 2 : 1
                    strokeStyle: ShapePath.DashLine
                    dashPattern: [5, 4]
                    fillColor: "transparent"

                    PathRectangle {
                        x: 0.5
                        y: 0.5
                        width: zone.width - 1
                        height: zone.height - 1
                        radius: 12
                    }
                }
            }

            ColumnLayout {
                id: dropContent
                anchors.centerIn: parent
                width: parent.width - 32
                spacing: 10

                Icon {
                    Layout.alignment: Qt.AlignHCenter
                    name: "upload"
                    size: 30
                    color: window.dropping ? Theme.accent : Theme.secondaryText
                }

                Text {
                    Layout.fillWidth: true
                    text: L10n.f("Drop files to send them to %@", [window.phoneName])
                    color: Theme.secondaryText
                    font.pixelSize: Theme.bodySize
                    horizontalAlignment: Text.AlignHCenter
                    wrapMode: Text.Wrap
                }

                PillButton {
                    Layout.alignment: Qt.AlignHCenter
                    prominent: true
                    text: L10n.t("Choose Files...")
                    onClicked: chooser.open()
                }
            }
        }

        // How the last send ended; one that needs the phone's permission offers to ask for it.
        ColumnLayout {
            Layout.fillWidth: true
            visible: window.outcome !== null && !window.sending
            spacing: 6

            RowLayout {
                Layout.alignment: Qt.AlignHCenter
                spacing: 6

                Rectangle {
                    Layout.preferredWidth: 18
                    Layout.preferredHeight: 18
                    radius: 9
                    color: window.outcome && window.outcome.succeeded ? window.success : "transparent"
                    border.width: window.outcome && window.outcome.succeeded ? 0 : 1.5
                    border.color: Theme.text
                    Accessible.ignored: true

                    Icon {
                        anchors.centerIn: parent
                        name: window.outcome && window.outcome.succeeded ? "checkmark" : "exclamation"
                        size: 16
                        color: window.outcome && window.outcome.succeeded ? "white" : Theme.text
                    }
                }

                Text {
                    text: window.outcome ? window.outcome.title : ""
                    color: window.outcome && window.outcome.succeeded ? window.success : Theme.text
                    font.pixelSize: Theme.bodySize + 1
                    font.weight: Font.DemiBold
                }
            }

            Text {
                Layout.fillWidth: true
                text: window.outcome ? window.outcome.message : ""
                color: Theme.secondaryText
                font.pixelSize: Theme.bodySize
                horizontalAlignment: Text.AlignHCenter
                wrapMode: Text.Wrap
            }

            PillButton {
                Layout.alignment: Qt.AlignHCenter
                visible: window.outcome !== null && window.outcome.needsPermission === true
                text: L10n.t("Send Request to Phone")
                onClicked: EngineBridge.perform("files.requestPermission", "")
            }
        }

        // A receive under way.
        RowLayout {
            Layout.alignment: Qt.AlignHCenter
            visible: !!window.transfers.receiveStatus
            spacing: 6

            Rectangle {
                Layout.preferredWidth: 18
                Layout.preferredHeight: 18
                radius: 9
                color: "transparent"
                border.width: 1.5
                border.color: Theme.secondaryText
                Accessible.ignored: true

                Icon {
                    anchors.centerIn: parent
                    name: "arrowDown"
                    size: 14
                    color: Theme.secondaryText
                }
            }

            Text {
                text: window.transfers.receiveStatus || ""
                color: Theme.secondaryText
                font.pixelSize: Theme.bodySize
            }
        }
    }

    FileDialog {
        id: chooser
        title: L10n.t("Send Files")
        fileMode: FileDialog.OpenFiles
        onAccepted: window.send(selectedFiles)
    }

    Dialog {
        id: alert

        property string heading
        property string message

        parent: Overlay.overlay
        anchors.centerIn: parent
        width: Math.min(360, window.width - 32)
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
                text: alert.heading
                color: Theme.text
                font.pixelSize: Theme.headlineSize - 2
                font.weight: Font.DemiBold
                wrapMode: Text.Wrap
            }

            Text {
                Layout.fillWidth: true
                visible: text.length > 0
                text: alert.message
                color: Theme.secondaryText
                font.pixelSize: Theme.bodySize
                wrapMode: Text.Wrap
            }

            PillButton {
                Layout.alignment: Qt.AlignRight
                Layout.topMargin: 8
                prominent: true
                text: L10n.t("OK")
                onClicked: alert.close()
            }
        }
    }
}
