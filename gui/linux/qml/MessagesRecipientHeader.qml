import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import PhoneLink

// The "To:" line of the new message — a field that turns names and numbers into recipients, with the
// phone's contacts suggested below the line — and, once messages were sent to them, the fixed
// recipients. Return takes the first suggestion, else the typed number or email address; Escape
// closes an empty new message.
ColumnLayout {
    id: header

    property var detail
    readonly property var recipients: detail ? detail.recipients : []
    readonly property bool canAdd: detail !== null && detail.canAddRecipient
    readonly property bool locked: detail !== null && detail.recipientsLocked
    property bool applying: false

    function focusField() {
        field.forceActiveFocus()
    }

    // What the section holds for the field: it clears the field once a recipient was added.
    function apply() {
        if (detail === null || field.text === detail.recipientQuery)
            return
        applying = true
        field.text = detail.recipientQuery
        applying = false
    }

    spacing: 0
    onDetailChanged: apply()

    Flow {
        id: flow

        readonly property Item lastItem: tokens.count > 0 ? tokens.itemAt(tokens.count - 1) : toLabel
        readonly property real used: lastItem ? lastItem.x + lastItem.width + spacing : 0

        Layout.fillWidth: true
        Layout.leftMargin: 20
        Layout.rightMargin: 20
        // One line of recipients is as tall as a conversation's header.
        Layout.topMargin: 13
        Layout.bottomMargin: 13
        spacing: 4

        Text {
            id: toLabel
            height: 26
            verticalAlignment: Text.AlignVCenter
            text: L10n.t("To:")
            color: Theme.secondaryText
            font.pixelSize: Theme.bodySize
        }

        Repeater {
            id: tokens
            model: header.recipients

            Rectangle {
                id: token
                required property var modelData
                width: tokenRow.implicitWidth + (header.locked ? 16 : 12)
                height: 26
                radius: 13
                color: Theme.controlBackground

                HoverHandler {
                    id: tokenHover
                }

                ToolTip.visible: tokenHover.hovered
                ToolTip.delay: 600
                ToolTip.text: token.modelData.address

                RowLayout {
                    id: tokenRow
                    anchors.verticalCenter: parent.verticalCenter
                    x: 8
                    spacing: 2

                    Text {
                        Layout.maximumWidth: flow.width - 60
                        text: token.modelData.title
                        color: Theme.text
                        font.pixelSize: Theme.bodySize
                        elide: Text.ElideRight
                    }

                    AbstractButton {
                        id: remove
                        visible: !header.locked
                        implicitWidth: 18
                        implicitHeight: 18
                        hoverEnabled: true
                        Accessible.role: Accessible.Button
                        Accessible.name: L10n.f("Remove %@", [token.modelData.title])
                        onClicked: EngineBridge.perform("messages.removeRecipient", token.modelData.address)

                        contentItem: Item {
                            Icon {
                                anchors.centerIn: parent
                                name: "close"
                                size: 10
                                color: remove.hovered ? Theme.text : Theme.secondaryText
                            }
                        }
                    }
                }
            }
        }

        // The rest of the last line, or a line of its own when less than a third is left.
        TextField {
            id: field
            visible: header.canAdd
            width: flow.width - flow.used >= flow.width / 3 ? flow.width - flow.used : flow.width
            height: 26
            leftPadding: 2
            rightPadding: 2
            topPadding: 0
            bottomPadding: 0
            verticalAlignment: TextInput.AlignVCenter
            font.pixelSize: Theme.bodySize
            color: Theme.text
            placeholderText: header.recipients.length === 0 ? L10n.t("Name or Number") : ""
            placeholderTextColor: Theme.secondaryText
            selectionColor: Theme.accent
            selectedTextColor: "white"
            background: null
            Accessible.name: L10n.t("To")
            onTextChanged: {
                if (!header.applying)
                    EngineBridge.perform("messages.recipientQuery", text)
            }
            Keys.onReturnPressed: EngineBridge.perform("messages.submitRecipient", "")
            Keys.onEnterPressed: EngineBridge.perform("messages.submitRecipient", "")
            Keys.onEscapePressed: event => {
                event.accepted = !header.detail.newMessageHasContent
                EngineBridge.perform("messages.cancelNewMessage", "")
            }
        }
    }

    RowLayout {
        Layout.fillWidth: true
        Layout.leftMargin: 20
        Layout.rightMargin: 20
        Layout.bottomMargin: 8
        visible: header.detail !== null && header.detail.contactsNotice !== null
        spacing: 8

        Text {
            Layout.fillWidth: true
            text: header.detail !== null && header.detail.contactsNotice === "turnOn"
                  ? L10n.t("Turn on your phone's contacts on this computer to search them")
                  : L10n.t("Allow access to your phone's contacts to search them")
            color: Theme.secondaryText
            font.pixelSize: Theme.subheadSize
            wrapMode: Text.Wrap
        }

        PillButton {
            visible: header.detail !== null && header.detail.contactsNotice === "allowOnPhone"
            enabled: header.detail !== null && !header.detail.requestingContacts
            text: L10n.t("Send Request to Phone")
            onClicked: EngineBridge.perform("messages.requestContactsPermission", "")
        }

        PillButton {
            visible: header.detail !== null && header.detail.contactsNotice === "turnOn"
            text: L10n.t("Turn On")
            onClicked: EngineBridge.perform("messages.turnOnContacts", "")
        }
    }
}
