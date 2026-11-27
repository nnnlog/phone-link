import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import QtQuick.Window
import PhoneLink

// The incoming-call window: a borderless window above the other windows, at the top right of the
// screen where the window system lets it place itself, with one card per ringing call. It asks for no
// keyboard focus when it appears and none of its buttons holds the focus until clicked or reached with
// Tab, so typing elsewhere never answers a call. Dragging a card moves the window; closing it hides
// every call it shows (the phone keeps ringing).
Window {
    id: window

    // The ringing calls it shows (query "callControl" `popup`).
    property var calls: []
    // The phone lets this computer send messages, for *Message*.
    property bool canMessage: false
    // A call answered here is heard here; otherwise the window says it is heard on the phone.
    property bool canTakeAudio: false

    readonly property string phoneName: EngineBridge.phoneName

    title: L10n.t("Incoming Call")
    flags: Qt.Window | Qt.FramelessWindowHint | Qt.WindowStaysOnTopHint
    // Its own window, not the main window's: it shows while the main window is hidden or minimized.
    transientParent: null
    color: "transparent"
    width: 376
    height: cards.implicitHeight + 16
    x: Screen.virtualX + Screen.width - width - 12
    y: Screen.virtualY + Math.max(12, Screen.height - Screen.desktopAvailableHeight + 8)
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

    onCallsChanged: cardModel.sync(calls, call => call.id)
    Component.onCompleted: cardModel.sync(calls, call => call.id)
    onClosing: close => {
        close.accepted = true
        window.calls.forEach(call => EngineBridge.perform("calls.hidePopup", String(call.id)))
    }

    CallsFormat {
        id: format
    }

    KeyedListModel {
        id: cardModel
    }

    ColumnLayout {
        id: cards

        anchors.fill: parent
        anchors.margins: 8
        spacing: 8

        Repeater {
            model: cardModel

            // One ringing call: who, on which phone it rings, that it is heard on the phone when this
            // computer cannot take its audio, and *Decline*, *Message* and *Answer*.
            Rectangle {
                id: card

                required property string json
                readonly property var call: JSON.parse(json)
                readonly property string shownName: format.activeName(call)
                readonly property bool canReply: call.number.length > 0 && window.canMessage

                function reply(text) {
                    EngineBridge.perform("calls.reply", JSON.stringify({ id: card.call.id, number: card.call.number, text: text }))
                }

                Layout.fillWidth: true
                implicitHeight: content.implicitHeight + 28
                radius: 14
                color: Theme.raisedBackground
                border.color: Theme.separator

                Accessible.role: Accessible.Grouping
                Accessible.name: L10n.f("Incoming call from %@", [shownName])

                DragHandler {
                    target: null
                    onActiveChanged: {
                        if (active)
                            window.startSystemMove()
                    }
                }

                ColumnLayout {
                    id: content

                    anchors.fill: parent
                    anchors.margins: 14
                    spacing: 10

                    RowLayout {
                        Layout.fillWidth: true
                        spacing: 12

                        Avatar {
                            Layout.alignment: Qt.AlignTop
                            name: card.call.name || ""
                            photo: card.call.photo || ""
                            size: 44
                        }

                        ColumnLayout {
                            Layout.fillWidth: true
                            spacing: 2

                            Text {
                                Layout.fillWidth: true
                                text: card.shownName
                                color: Theme.text
                                font.pixelSize: Theme.bodySize + 2
                                font.weight: Font.Bold
                                elide: Text.ElideRight
                            }

                            Text {
                                Layout.fillWidth: true
                                text: card.call.waiting ? L10n.t("Call Waiting")
                                    : window.phoneName.length > 0 ? L10n.f("Incoming call on %@", [window.phoneName])
                                    : L10n.t("Incoming Call")
                                color: Theme.secondaryText
                                font.pixelSize: Theme.subheadSize
                                elide: Text.ElideRight
                            }
                        }

                        IconButton {
                            Layout.alignment: Qt.AlignTop
                            implicitWidth: 24
                            implicitHeight: 24
                            iconName: "close"
                            label: L10n.t("Hide. Your phone keeps ringing.")
                            onClicked: EngineBridge.perform("calls.hidePopup", String(card.call.id))
                        }
                    }

                    // Only where this computer cannot take the call's audio.
                    RowLayout {
                        Layout.fillWidth: true
                        visible: !window.canTakeAudio
                        spacing: 8

                        Icon {
                            Layout.alignment: Qt.AlignTop
                            name: "handset"
                            size: 15
                            color: Theme.secondaryText
                        }

                        Text {
                            Layout.fillWidth: true
                            text: L10n.t("If you answer on this computer, you'll talk on your phone.")
                            color: Theme.secondaryText
                            font.pixelSize: Theme.bodySize
                            font.weight: Font.Medium
                            wrapMode: Text.Wrap
                        }
                    }

                    RowLayout {
                        Layout.fillWidth: true
                        spacing: 8

                        CallActionButton {
                            Layout.fillWidth: true
                            Layout.preferredWidth: 1
                            fill: Theme.destructive
                            iconName: "phoneDown"
                            text: L10n.t("Decline")
                            onClicked: EngineBridge.perform("calls.decline", String(card.call.id))
                        }

                        AbstractButton {
                            id: messageButton

                            visible: card.canReply
                            implicitWidth: 44
                            implicitHeight: 34
                            hoverEnabled: true
                            focusPolicy: Qt.StrongFocus
                            Accessible.role: Accessible.ButtonMenu
                            Accessible.name: L10n.t("Message")
                            Accessible.description: L10n.t("Decline and send a message")
                            ToolTip.visible: hovered && !replies.visible
                            ToolTip.delay: 600
                            ToolTip.text: L10n.t("Decline and send a message")
                            onClicked: replies.popup(messageButton, 0, messageButton.height + 4)

                            background: Rectangle {
                                radius: height / 2
                                color: messageButton.hovered ? Theme.controlHover : Theme.controlBackground
                                border.color: messageButton.visualFocus ? Theme.accent : "transparent"
                            }

                            contentItem: Item {
                                Icon {
                                    anchors.centerIn: parent
                                    name: "message"
                                    filled: true
                                    size: 16
                                    color: Theme.text
                                }
                            }

                            CallMenu {
                                id: replies

                                // Its own window: the incoming-call window is too small to hold it. Qt before
                                // 6.8 has no popupType and keeps the menu inside the window.
                                Component.onCompleted: {
                                    if (replies.popupType !== undefined)
                                        replies.popupType = Popup.Window
                                }

                                CallMenuItem {
                                    text: L10n.t("Sorry, I can't talk right now.")
                                    onTriggered: card.reply(text)
                                }

                                CallMenuItem {
                                    text: L10n.t("I'll call you back in a minute.")
                                    onTriggered: card.reply(text)
                                }

                                CallMenuItem {
                                    text: L10n.t("I'm busy. Call me back later.")
                                    onTriggered: card.reply(text)
                                }

                                CallMenuSeparator {}

                                CallMenuItem {
                                    text: L10n.t("Custom Message...")
                                    onTriggered: {
                                        EngineBridge.perform("calls.decline", String(card.call.id))
                                        EngineBridge.perform("messages.compose", JSON.stringify({ addresses: [card.call.number], text: "" }))
                                    }
                                }
                            }
                        }

                        CallActionButton {
                            Layout.fillWidth: true
                            Layout.preferredWidth: 1
                            fill: Theme.dark ? "#30d158" : "#34c759"
                            iconName: "phone"
                            text: L10n.t("Answer")
                            onClicked: EngineBridge.perform("calls.answer", String(card.call.id))
                        }
                    }
                }
            }
        }
    }
}
