import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import QtQuick.Window
import PhoneLink

// The question of a Bluetooth pairing Phone Link started to reach the phone (for calls or the
// hotspot): the code this computer and the phone both show. *Pair* (Return) confirms it; *Cancel*
// (Escape) or closing the window refuses it. The phone asks its user the same. It shows above the other
// windows, whether or not the main window does.
Window {
    id: window

    // The six-digit code (the `bluetoothPairing` event's `code`).
    property string code: ""

    // The user answered; the host then closes the window.
    signal answered(bool confirmed)

    title: L10n.t("Bluetooth")
    flags: Qt.Dialog | Qt.WindowStaysOnTopHint
    // Its own window, not the main window's: it shows while the main window is hidden or minimized.
    transientParent: null
    width: 400
    height: body.implicitHeight + 48
    minimumWidth: width
    maximumWidth: width
    color: Theme.windowBackground
    visible: true

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

    Component.onCompleted: requestActivate()
    onClosing: close => {
        close.accepted = true
        window.answered(false)
    }

    Shortcut {
        sequence: "Escape"
        onActivated: window.answered(false)
    }

    ColumnLayout {
        id: body

        anchors.left: parent.left
        anchors.right: parent.right
        anchors.top: parent.top
        anchors.margins: 24
        spacing: 12
        focus: true
        Keys.onReturnPressed: window.answered(true)
        Keys.onEnterPressed: window.answered(true)

        Icon {
            Layout.alignment: Qt.AlignHCenter
            name: "phoneConnection"
            size: 40
            color: Theme.accent
        }

        Text {
            Layout.fillWidth: true
            horizontalAlignment: Text.AlignHCenter
            text: L10n.t("Pair with Your Phone?")
            color: Theme.text
            font.pixelSize: Theme.titleSize - 2
            font.weight: Font.DemiBold
            wrapMode: Text.Wrap
            Accessible.role: Accessible.Heading
        }

        Text {
            Layout.fillWidth: true
            horizontalAlignment: Text.AlignHCenter
            text: window.code
            color: Theme.text
            font.pixelSize: 34
            font.weight: Font.DemiBold
            font.letterSpacing: 4
        }

        Text {
            Layout.fillWidth: true
            horizontalAlignment: Text.AlignHCenter
            text: L10n.f("Make sure your phone shows the code %@, then confirm on both devices.", [window.code])
            color: Theme.secondaryText
            font.pixelSize: Theme.bodySize
            wrapMode: Text.Wrap
        }

        RowLayout {
            Layout.fillWidth: true
            Layout.topMargin: 8

            PillButton {
                text: L10n.t("Cancel")
                onClicked: window.answered(false)
            }

            Item {
                Layout.fillWidth: true
            }

            PillButton {
                prominent: true
                text: L10n.t("Pair")
                onClicked: window.answered(true)
            }
        }
    }
}
