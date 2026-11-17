import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import QtQuick.Window
import PhoneLink

// The window that pairs a new phone (from Settings › Account and the sidebar's phone menu); closing it
// ends the session. A paired phone becomes the phone in use, and the window closes.
Window {
    id: window

    title: L10n.t("Pair a New Phone")
    width: 480
    height: 560
    minimumWidth: 420
    minimumHeight: 420
    color: Theme.windowBackground

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

    onClosing: EngineBridge.perform("pairing.cancel", "")

    Shortcut {
        sequence: "Escape"
        onActivated: window.close()
    }

    Flickable {
        id: scroller
        anchors.fill: parent
        contentWidth: width
        contentHeight: Math.max(height, pairing.implicitHeight + 56)
        boundsBehavior: Flickable.StopAtBounds
        clip: true
        ScrollBar.vertical: ScrollBar {
            policy: scroller.contentHeight > scroller.height ? ScrollBar.AsNeeded : ScrollBar.AlwaysOff
        }

        PairingView {
            id: pairing
            x: 28
            y: (scroller.contentHeight - implicitHeight) / 2
            width: scroller.width - 56
            cancellable: true
            onCancelled: window.close()
        }
    }
}
