import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import PhoneLink

// The Calls list's keypad: a number typed or tapped in, or taken from a `tel:` link, and *Call*, which
// places the call on the phone.
// The number clears once the phone placed the call; it stays after a failure.
Popup {
    id: pad

    // Call control's state (query "callControl").
    property var control: ({})
    // The number last sent to the phone, while the dial runs.
    property string pending

    readonly property bool canCall: field.text.length > 0 && control.canDial === true
    readonly property var keys: [["1", ""], ["2", "ABC"], ["3", "DEF"], ["4", "GHI"], ["5", "JKL"], ["6", "MNO"],
                                 ["7", "PQRS"], ["8", "TUV"], ["9", "WXYZ"], ["*", ""], ["0", "+"], ["#", ""]]

    // Opens the keypad with `number` in its field, for the user to call.
    function openWith(number) {
        open()
        field.text = number
    }

    function call() {
        if (!canCall)
            return
        pending = field.text
        EngineBridge.perform("calls.dial", field.text)
    }

    padding: 16
    width: 260
    focus: true
    closePolicy: Popup.CloseOnEscape | Popup.CloseOnPressOutsideParent
    onOpened: field.forceActiveFocus()

    background: Rectangle {
        radius: Theme.moduleRadius
        color: Theme.raisedBackground
        border.color: Theme.separator
    }

    Connections {
        target: EngineBridge

        function onAreaEvent(area, name, payload) {
            if (area !== "calls" || name !== "dialed" || pad.pending.length === 0)
                return
            const dialed = JSON.parse(payload)
            if (dialed.placed && dialed.number === pad.pending && field.text === pad.pending)
                field.text = ""
            pad.pending = ""
        }
    }

    contentItem: ColumnLayout {
        spacing: 14

        RowLayout {
            Layout.fillWidth: true
            spacing: 6

            TextField {
                id: field

                Layout.fillWidth: true
                placeholderText: L10n.t("Phone number")
                font.pixelSize: Theme.headlineSize
                // Qt before 6.6 has no font.features and keeps proportional digits.
                Component.onCompleted: {
                    if (font.features !== undefined)
                        font.features = { "tnum": 1 }
                }
                color: Theme.text
                placeholderTextColor: Theme.secondaryText
                selectionColor: Theme.accent
                selectedTextColor: "white"
                inputMethodHints: Qt.ImhDialableCharactersOnly
                leftPadding: 10
                rightPadding: 10
                Accessible.name: L10n.t("Phone number")
                onAccepted: pad.call()

                background: Rectangle {
                    implicitHeight: 34
                    radius: Theme.rowRadius
                    color: Theme.fieldBackground
                    border.color: field.activeFocus ? Theme.accent : Theme.fieldBorder
                }
            }

            IconButton {
                iconName: "deleteLeft"
                label: L10n.t("Delete")
                enabled: field.text.length > 0
                onClicked: field.text = field.text.slice(0, -1)
            }
        }

        GridLayout {
            Layout.alignment: Qt.AlignHCenter
            columns: 3
            columnSpacing: 12
            rowSpacing: 10

            Repeater {
                model: pad.keys

                AbstractButton {
                    id: key

                    required property var modelData

                    implicitWidth: 64
                    implicitHeight: 48
                    hoverEnabled: true
                    // Tab reaches the keys; a click leaves the focus in the number.
                    focusPolicy: Qt.TabFocus
                    Accessible.role: Accessible.Button
                    Accessible.name: modelData[0]
                    onClicked: field.text += modelData[0]

                    background: Rectangle {
                        radius: 10
                        color: key.down ? Theme.selection : key.hovered ? Theme.controlHover : Theme.controlBackground
                        border.color: key.visualFocus ? Theme.accent : "transparent"
                    }

                    contentItem: Item {
                        Column {
                            anchors.centerIn: parent

                            Text {
                                anchors.horizontalCenter: parent.horizontalCenter
                                text: key.modelData[0]
                                color: Theme.text
                                font.pixelSize: 22
                            }

                            // Every key keeps the letters' line, so the digits line up.
                            Text {
                                anchors.horizontalCenter: parent.horizontalCenter
                                text: key.modelData[1].length > 0 ? key.modelData[1] : " "
                                color: Theme.secondaryText
                                font.pixelSize: 9
                                font.letterSpacing: 1
                            }
                        }
                    }
                }
            }
        }

        CallActionButton {
            Layout.fillWidth: true
            fill: Theme.dark ? "#30d158" : "#34c759"
            iconName: "phone"
            text: L10n.t("Call")
            enabled: pad.canCall
            onClicked: pad.call()
        }

        Text {
            Layout.fillWidth: true
            horizontalAlignment: Text.AlignHCenter
            text: L10n.t("Calls go through your phone.")
            color: Theme.secondaryText
            font.pixelSize: Theme.captionSize
            wrapMode: Text.Wrap
        }
    }
}
