import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Templates as T
import QtQuick.Layouts
import QtQuick.Window
import PhoneLink

// The call window: who the call placed or answered on this computer is with, where it is heard, its
// state and how long it has run, and the other call below it, which a click swaps in. Its controls
// are *Mute*, *Keypad* and *Hold*, which act while the call is heard on this computer, the transfer
// of the audio to the phone or back, and *End* (Ctrl+Shift+H); Ctrl+Shift+M mutes and unmutes. Closing
// the window asks whether to end the calls. Call-control failures show here while it is open.
Window {
    id: window

    // The main call and the other call (query "callControl" `primary` and `secondary`).
    property var primary: null
    property var secondary: null
    // Whether this computer has a microphone and a sound output (query "callControl").
    property bool hasMicrophone: true
    property bool hasOutput: true
    property double now: Date.now()

    readonly property var shownPrimary: primary || { id: 0, name: null, number: "", photo: null, status: "talking", talkingSinceMs: null }
    readonly property bool onComputer: shownPrimary.onComputer === true
    // The keypad shows in place of the call's picture and name.
    property bool keypadShown: false
    readonly property var keys: ["1", "2", "3", "4", "5", "6", "7", "8", "9", "*", "0", "#"]

    function perform(action) {
        if (primary !== null)
            EngineBridge.perform(action, String(primary.id))
    }

    onPrimaryChanged: {
        if (primary === null || primary.canUseKeypad !== true)
            keypadShown = false
    }

    title: onComputer ? L10n.t("Call on This Computer") : L10n.t("Call on Phone")
    flags: Qt.Window | Qt.WindowStaysOnTopHint
    // Its own window, not the main window's: it shows while the main window is hidden or minimized.
    transientParent: null
    width: 320
    height: body.implicitHeight + 40
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

    // The window closes with its calls: closing it asks whether to end them.
    onClosing: close => {
        close.accepted = false
        endQuestion.open()
    }

    CallsFormat {
        id: format
    }

    Timer {
        interval: 1000
        repeat: true
        running: window.shownPrimary.status === "talking" && window.shownPrimary.talkingSinceMs !== null
        triggeredOnStart: true
        onTriggered: window.now = Date.now()
    }

    Shortcut {
        sequence: "Ctrl+Shift+H"
        enabled: window.primary !== null
        onActivated: EngineBridge.perform("calls.end", String(window.primary.id))
    }

    Shortcut {
        sequence: "Ctrl+Shift+M"
        enabled: window.shownPrimary.canMute === true
        onActivated: window.perform("calls.toggleMute")
    }

    Connections {
        target: EngineBridge

        function onAreaEvent(area, name, payload) {
            if (area !== "calls" || name !== "failure")
                return
            const failure = JSON.parse(payload)
            alert.heading = failure.title
            alert.message = failure.message
            alert.open()
        }
    }

    ColumnLayout {
        id: body

        anchors.left: parent.left
        anchors.right: parent.right
        anchors.top: parent.top
        anchors.margins: 20
        spacing: 16

        // The main call.
        ColumnLayout {
            Layout.fillWidth: true
            spacing: 8

            Avatar {
                Layout.alignment: Qt.AlignHCenter
                visible: !window.keypadShown
                name: window.shownPrimary.name || ""
                photo: window.shownPrimary.photo || ""
                size: 72
            }

            // The keypad: each key is sent into the call, and its tone plays here.
            GridLayout {
                Layout.alignment: Qt.AlignHCenter
                visible: window.keypadShown
                columns: 3
                columnSpacing: 10
                rowSpacing: 8

                Repeater {
                    model: window.keys

                    AbstractButton {
                        id: key

                        required property string modelData

                        implicitWidth: 56
                        implicitHeight: 40
                        hoverEnabled: true
                        enabled: window.shownPrimary.canUseKeypad === true
                        Accessible.role: Accessible.Button
                        Accessible.name: modelData
                        onClicked: EngineBridge.perform("calls.key", JSON.stringify({ id: window.shownPrimary.id, key: modelData }))

                        background: Rectangle {
                            radius: 10
                            color: key.down ? Theme.selection : key.hovered ? Theme.controlHover : Theme.controlBackground
                            border.color: key.visualFocus ? Theme.accent : "transparent"
                        }

                        contentItem: Text {
                            horizontalAlignment: Text.AlignHCenter
                            verticalAlignment: Text.AlignVCenter
                            text: key.modelData
                            color: Theme.text
                            font.pixelSize: 20
                        }
                    }
                }
            }

            Text {
                Layout.fillWidth: true
                horizontalAlignment: Text.AlignHCenter
                text: format.activeName(window.shownPrimary)
                color: Theme.text
                font.pixelSize: Theme.headlineSize + 1
                font.weight: Font.DemiBold
                wrapMode: Text.Wrap
                maximumLineCount: 2
                elide: Text.ElideRight
            }

            Text {
                Layout.fillWidth: true
                visible: text.length > 0
                horizontalAlignment: Text.AlignHCenter
                text: format.activeNumber(window.shownPrimary)
                color: Theme.secondaryText
                font.pixelSize: Theme.bodySize + 1
            }

            ColumnLayout {
                Layout.fillWidth: true
                spacing: 2

                Text {
                    Layout.fillWidth: true
                    horizontalAlignment: Text.AlignHCenter
                    text: {
                        switch (window.shownPrimary.status) {
                        case "dialing": return L10n.t("Calling...")
                        case "held": return L10n.t("On hold")
                        case "incoming": return L10n.t("Incoming Call")
                        default:
                            return window.shownPrimary.muted === true ? L10n.t("Muted")
                                 : window.onComputer ? L10n.t("On this computer") : L10n.t("On phone")
                        }
                    }
                    color: Theme.secondaryText
                    font.pixelSize: Theme.bodySize + 1
                }

                Text {
                    Layout.fillWidth: true
                    visible: window.shownPrimary.status === "talking" && window.shownPrimary.talkingSinceMs !== null
                    horizontalAlignment: Text.AlignHCenter
                    text: visible ? format.elapsed(window.shownPrimary.talkingSinceMs, window.now) : ""
                    color: Theme.secondaryText
                    font.pixelSize: Theme.bodySize + 1
                    // Qt before 6.6 has no font.features and keeps proportional digits.
                    Component.onCompleted: {
                        if (font.features !== undefined)
                            font.features = { "tnum": 1 }
                    }
                }
            }

            RowLayout {
                Layout.alignment: Qt.AlignHCenter
                Layout.topMargin: 8
                spacing: 4

                CallControlButton {
                    iconName: window.shownPrimary.muted === true ? "microphoneSlash" : "microphone"
                    text: window.shownPrimary.muted === true ? L10n.t("Unmute") : L10n.t("Mute")
                    active: window.shownPrimary.muted === true
                    enabled: window.shownPrimary.canMute === true
                    onClicked: window.perform("calls.toggleMute")
                }

                CallControlButton {
                    iconName: "keypad"
                    text: L10n.t("Keypad")
                    active: window.keypadShown
                    enabled: window.shownPrimary.canUseKeypad === true
                    onClicked: window.keypadShown = !window.keypadShown
                }

                CallControlButton {
                    iconName: window.shownPrimary.status === "held" ? "play" : "pause"
                    text: window.shownPrimary.status === "held" ? L10n.t("Resume") : L10n.t("Hold")
                    active: window.shownPrimary.status === "held"
                    enabled: window.shownPrimary.canHold === true
                    onClicked: window.perform("calls.toggleHold")
                }

                CallControlButton {
                    iconName: window.onComputer ? "handset" : "computer"
                    text: window.onComputer ? L10n.t("To Phone") : L10n.t("To Computer")
                    help: window.onComputer ? L10n.t("Transfer to Phone") : L10n.t("Transfer to Computer")
                    enabled: window.shownPrimary.canTransfer === true
                    onClicked: window.perform("calls.transfer")
                }
            }

            CallActionButton {
                Layout.alignment: Qt.AlignHCenter
                Layout.topMargin: 6
                Layout.preferredWidth: 120
                fill: Theme.destructive
                iconName: "phoneDown"
                text: L10n.t("End")
                help: L10n.t("End call")
                enabled: window.primary !== null
                onClicked: EngineBridge.perform("calls.end", String(window.primary.id))
            }
        }

        Rectangle {
            Layout.fillWidth: true
            visible: window.secondary !== null
            implicitHeight: 1
            color: Theme.separator
        }

        // The other call: on hold; a click swaps the two.
        AbstractButton {
            id: other

            readonly property var call: window.secondary || { name: null, number: "", photo: null, status: "talking" }

            Layout.fillWidth: true
            visible: window.secondary !== null
            enabled: other.call.status === "held"
            hoverEnabled: true
            implicitHeight: otherRow.implicitHeight + 8
            Accessible.role: Accessible.Button
            Accessible.name: format.activeName(other.call)
            Accessible.description: otherStatus.text
            onClicked: EngineBridge.perform("calls.swap", "")

            background: Rectangle {
                radius: Theme.rowRadius
                color: other.hovered ? Theme.hover : "transparent"
                border.color: other.visualFocus ? Theme.accent : "transparent"
            }

            contentItem: RowLayout {
                id: otherRow

                spacing: 10

                Avatar {
                    name: other.call.name || ""
                    photo: other.call.photo || ""
                    size: 32
                }

                ColumnLayout {
                    Layout.fillWidth: true
                    spacing: 1

                    Text {
                        Layout.fillWidth: true
                        text: format.activeName(other.call)
                        color: Theme.text
                        font.pixelSize: Theme.bodySize + 1
                        elide: Text.ElideRight
                    }

                    Text {
                        id: otherStatus

                        Layout.fillWidth: true
                        text: other.call.status === "held" ? L10n.t("On hold. Click to switch to this call.") : L10n.t("On phone")
                        color: Theme.secondaryText
                        font.pixelSize: Theme.captionSize
                        wrapMode: Text.Wrap
                    }
                }
            }
        }

        Rectangle {
            Layout.fillWidth: true
            visible: !window.onComputer
            implicitHeight: 1
            color: Theme.separator
        }

        // While the call is heard on the phone.
        RowLayout {
            Layout.fillWidth: true
            visible: !window.onComputer
            spacing: 8

            Icon {
                Layout.alignment: Qt.AlignTop
                name: "handset"
                size: 15
                color: Theme.secondaryText
            }

            Text {
                Layout.fillWidth: true
                text: !window.hasMicrophone ? L10n.t("Talk on your phone. This computer has no microphone.")
                    : !window.hasOutput ? L10n.t("Talk on your phone. This computer has no sound output.")
                    : L10n.t("Talk on your phone. This computer only controls the call.")
                color: Theme.secondaryText
                font.pixelSize: Theme.bodySize
                wrapMode: Text.Wrap
            }
        }
    }

    // The question a close of the window asks: ending the calls, or keeping the window.
    Dialog {
        id: endQuestion

        readonly property bool several: window.secondary !== null

        parent: T.Overlay.overlay
        anchors.centerIn: parent
        width: parent ? parent.width - 32 : 288
        modal: true
        focus: true
        padding: 18

        background: Rectangle {
            radius: Theme.moduleRadius
            color: Theme.raisedBackground
            border.color: Theme.separator
        }

        contentItem: ColumnLayout {
            spacing: 10

            Text {
                Layout.fillWidth: true
                text: endQuestion.several ? L10n.t("Are you sure you want to end all calls?")
                                          : L10n.t("Are you sure you want to end the call?")
                color: Theme.text
                font.pixelSize: Theme.headlineSize - 2
                font.weight: Font.DemiBold
                wrapMode: Text.Wrap
            }

            Text {
                Layout.fillWidth: true
                text: L10n.t("You can minimize the window instead.")
                color: Theme.secondaryText
                font.pixelSize: Theme.bodySize
                wrapMode: Text.Wrap
            }

            RowLayout {
                Layout.fillWidth: true
                Layout.topMargin: 4

                PillButton {
                    text: L10n.t("Cancel")
                    onClicked: endQuestion.close()
                }

                Item {
                    Layout.fillWidth: true
                }

                PillButton {
                    prominent: true
                    text: endQuestion.several ? L10n.t("End All Calls") : L10n.t("End Call")
                    onClicked: {
                        endQuestion.close()
                        EngineBridge.perform("calls.endAll", "")
                    }
                }
            }
        }
    }

    // A call-control failure while the window is open.
    Dialog {
        id: alert

        property string heading
        property string message

        parent: T.Overlay.overlay
        anchors.centerIn: parent
        width: parent ? parent.width - 32 : 288
        modal: true
        focus: true
        padding: 18

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
                Layout.topMargin: 4
                prominent: true
                text: L10n.t("OK")
                onClicked: alert.close()
            }
        }
    }
}
