import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import PhoneLink

// The Calls section's list column: the phone's recent calls under their days, searched by name or by
// the number's digits — or the state that stands in for them: call history off on this computer, a
// phone whose Link to Windows needs an update, notifications or call history not allowed on the phone,
// calls on their way, or no calls at all. Above them, what call control needs from the user or why the
// last dial failed; in the header, the keypad.
//
// It tells the detail column what it shows (areaEvent "calls" "listShown": {content, selectedId}).
Rectangle {
    id: column

    property var calls: []
    property double selectedId: -1

    readonly property var callsState: { EngineBridge.callsRevision; return JSON.parse(EngineBridge.query("callsState", "")) }
    readonly property var control: { EngineBridge.callsRevision; return JSON.parse(EngineBridge.query("callControl", "")) || {} }
    readonly property string access: callsState ? callsState.access : ""
    // What the column shows: "nothing", "turnedOff", "updateRequired", "needsNotifications",
    // "needsPhoneAccess", "updating", "empty" or "list".
    readonly property string content: {
        if (callsState === null)
            return "nothing"
        switch (callsState.access) {
        case "consentRequired": return "turnedOff"
        case "callLogsUnsupported": return "updateRequired"
        case "postNotificationRequired": return "needsNotifications"
        case "permissionRequired": return "needsPhoneAccess"
        case "granted": return calls.length > 0 ? "list" : callsState.updating ? "updating" : "empty"
        default: return "nothing"
        }
    }
    readonly property string phoneName: EngineBridge.phoneName.length > 0 ? EngineBridge.phoneName : L10n.t("your phone")
    readonly property var rows: format.rows(calls, search.text)
    readonly property string notice: control.dialNotice || control.connectionNotice || ""

    function tellDetail() {
        EngineBridge.areaEvent("calls", "listShown", JSON.stringify({ content: content, selectedId: selectedId }))
    }

    // The keypad, or switching call control on (explaining first, the first time).
    function keypad() {
        if (control.enabled === true)
            dialPad.open()
        else if (intro.needed())
            intro.open()
        else
            EngineBridge.perform("calls.enable", "true")
    }

    function openMenu(call, item) {
        menu.call = call
        if (item)
            menu.popup(item, item.width / 2, item.height / 2)
        else
            menu.popup()
    }

    color: Theme.listBackground
    onRowsChanged: rowModel.sync(rows, row => row.key)
    onContentChanged: tellDetail()
    onSelectedIdChanged: tellDetail()
    // A call that is gone is no longer selected.
    onCallsChanged: {
        if (selectedId >= 0 && !calls.some(call => call.id === selectedId))
            selectedId = -1
    }
    onAccessChanged: {
        if (access !== "postNotificationRequired")
            notificationHelp.close()
    }
    Component.onCompleted: {
        rowModel.sync(rows, row => row.key)
        // After the detail column exists.
        Qt.callLater(tellDetail)
    }

    CallsFormat {
        id: format
    }

    CallClipboard {
        id: clipboard
    }

    KeyedListModel {
        id: rowModel
    }

    CallControlIntroDialog {
        id: intro
    }

    NotificationPermissionHelp {
        id: notificationHelp
        phoneName: column.phoneName
    }

    Shortcut {
        sequence: StandardKey.Find
        enabled: column.visible && column.content === "list"
        onActivated: search.forceActiveFocus()
    }

    // What can be done with a call: call its number back on the phone, write to it, show its contact,
    // copy the number.
    CallMenu {
        id: menu

        property var call: ({ number: "", contactId: null })

        CallMenuItem {
            text: L10n.t("Call")
            visible: column.control.enabled === true && column.control.pcConsent === true
            enabled: visible && menu.call.number.length > 0 && column.control.canDial === true
            onTriggered: EngineBridge.perform("calls.dial", menu.call.number)
        }

        CallMenuItem {
            text: L10n.t("Send Message")
            enabled: menu.call.number.length > 0
            onTriggered: EngineBridge.perform("messages.compose", JSON.stringify({ addresses: [menu.call.number], text: "" }))
        }

        CallMenuItem {
            text: L10n.t("Show Contact")
            visible: menu.call.contactId !== null && menu.call.contactId !== undefined
            enabled: visible
            onTriggered: EngineBridge.perform("contacts.show", String(menu.call.contactId))
        }

        CallMenuSeparator {}

        CallMenuItem {
            text: L10n.t("Copy Number")
            enabled: menu.call.number.length > 0
            onTriggered: clipboard.copy(menu.call.number)
        }
    }

    ColumnLayout {
        anchors.fill: parent
        spacing: 0

        RowLayout {
            Layout.fillWidth: true
            Layout.preferredHeight: 52
            Layout.leftMargin: 20
            Layout.rightMargin: 10
            spacing: 8

            Text {
                Layout.fillWidth: true
                text: L10n.t("Calls")
                color: Theme.text
                font.pixelSize: Theme.headlineSize
                font.weight: Font.Bold
            }

            BusyIndicator {
                visible: column.content === "list" && column.callsState !== null && column.callsState.updating === true
                running: visible
                padding: 1
                implicitWidth: 20
                implicitHeight: 20
                Accessible.name: L10n.t("Updating call history...")
                ToolTip.visible: updatingHover.hovered
                ToolTip.delay: 600
                ToolTip.text: L10n.t("Updating call history...")

                HoverHandler {
                    id: updatingHover
                }
            }

            IconButton {
                id: keypadButton

                visible: column.control.offered === true && column.control.pcConsent === true
                iconName: "keypad"
                label: column.control.enabled === true ? L10n.t("Call a number using your phone")
                                                       : L10n.t("Turn on calls on this computer")
                active: dialPad.opened
                onClicked: column.keypad()

                DialPad {
                    id: dialPad
                    x: keypadButton.width - width
                    y: keypadButton.height + 6
                    control: column.control
                }
            }
        }

        CapsuleField {
            id: search

            Layout.fillWidth: true
            Layout.leftMargin: 12
            Layout.rightMargin: 12
            Layout.bottomMargin: 8
            visible: column.content === "list"
            placeholderText: L10n.t("Search")
            submittable: false
            Accessible.name: L10n.t("Search")
            Keys.onEscapePressed: text = ""
        }

        CallNotice {
            Layout.fillWidth: true
            Layout.leftMargin: 10
            Layout.rightMargin: 10
            Layout.bottomMargin: 8
            visible: column.notice.length > 0
            kind: column.notice
            connecting: column.control.connecting === true
        }

        ListView {
            id: list

            Layout.fillWidth: true
            Layout.fillHeight: true
            visible: column.content === "list"
            clip: true
            model: rowModel
            leftMargin: 10
            rightMargin: 10
            bottomMargin: 10
            ScrollBar.vertical: ScrollBar {}

            delegate: Loader {
                required property string json
                readonly property var modelData: JSON.parse(json)
                width: ListView.view.width - 20
                sourceComponent: modelData.header !== undefined ? headerRow : callRow

                Component {
                    id: headerRow

                    Item {
                        implicitHeight: 34

                        Text {
                            anchors.left: parent.left
                            anchors.leftMargin: 6
                            anchors.bottom: parent.bottom
                            anchors.bottomMargin: 6
                            text: modelData.header
                            color: Theme.secondaryText
                            font.pixelSize: Theme.subheadSize
                            font.weight: Font.DemiBold
                            Accessible.role: Accessible.Heading
                            Accessible.name: text
                        }

                        Rectangle {
                            anchors.left: parent.left
                            anchors.right: parent.right
                            anchors.bottom: parent.bottom
                            height: 1
                            color: Theme.separator
                        }
                    }
                }

                Component {
                    id: callRow

                    AbstractButton {
                        id: row

                        readonly property var call: modelData.call
                        readonly property var style: format.style(call.kind)
                        readonly property string time: format.rowTime(call.startMs)

                        implicitHeight: 52
                        hoverEnabled: true
                        Accessible.role: Accessible.Button
                        Accessible.name: [format.name(call), style.label, time].filter(part => part.length > 0).join(", ")
                        onClicked: column.selectedId = call.id
                        Keys.onMenuPressed: column.openMenu(row.call, row)

                        TapHandler {
                            acceptedButtons: Qt.RightButton
                            onTapped: column.openMenu(row.call, null)
                        }

                        background: Rectangle {
                            radius: Theme.rowRadius
                            color: column.selectedId === row.call.id ? Theme.selection
                                 : row.hovered ? Theme.hover : "transparent"
                            border.color: row.visualFocus ? Theme.accent : "transparent"
                        }

                        contentItem: RowLayout {
                            spacing: 10

                            Avatar {
                                Layout.leftMargin: 6
                                name: row.call.name || ""
                                photo: row.call.photo || ""
                                size: 36
                            }

                            ColumnLayout {
                                Layout.fillWidth: true
                                spacing: 2

                                Text {
                                    Layout.fillWidth: true
                                    text: format.name(row.call)
                                    color: row.style.missed ? Theme.destructive : Theme.text
                                    font.pixelSize: Theme.bodySize + 1
                                    elide: Text.ElideRight
                                }

                                RowLayout {
                                    Layout.fillWidth: true
                                    visible: row.style.icon.length > 0
                                    spacing: 4

                                    Icon {
                                        name: row.style.icon
                                        size: 12
                                        color: row.style.missed ? Theme.destructive : Theme.secondaryText
                                    }

                                    Text {
                                        Layout.fillWidth: true
                                        text: row.style.label
                                        color: row.style.missed ? Theme.destructive : Theme.secondaryText
                                        font.pixelSize: Theme.subheadSize
                                        elide: Text.ElideRight
                                    }
                                }
                            }

                            Text {
                                Layout.rightMargin: 6
                                text: row.time
                                color: Theme.secondaryText
                                font.pixelSize: Theme.subheadSize
                            }
                        }
                    }
                }
            }

            EmptyState {
                anchors.centerIn: parent
                width: parent.width - 40
                visible: column.rows.length === 0
                iconName: "search"
                title: L10n.f("No Results for “%@”", [search.text.trim()])
                description: L10n.t("Check the spelling or try a new search.")
            }
        }

        // What stands in for the list.
        Item {
            Layout.fillWidth: true
            Layout.fillHeight: true
            visible: column.content !== "list"

            ColumnLayout {
                anchors.centerIn: parent
                width: parent.width - 40
                visible: column.content !== "nothing" && column.content !== "updating"
                spacing: 14

                EmptyState {
                    Layout.fillWidth: true
                    iconName: column.content === "updateRequired" ? "save" : "phone"
                    title: {
                        switch (column.content) {
                        case "turnedOff": return L10n.t("Call History Is Off on This Computer")
                        case "updateRequired": return L10n.t("Update Link to Windows")
                        case "needsNotifications": return L10n.t("Allow Notifications on Your Phone")
                        case "needsPhoneAccess": return L10n.t("Allow Call History on Your Phone")
                        default: return L10n.t("No Recent Calls")
                        }
                    }
                    description: {
                        switch (column.content) {
                        case "turnedOff":
                            return L10n.t("Turn on call history to see your phone's recent calls on this computer.")
                        case "updateRequired":
                            return L10n.f("To see recent calls, update Link to Windows on %@, then reconnect.", [column.phoneName])
                        case "needsNotifications":
                            return L10n.f("Allow Link to Windows to send notifications on %@ to see your recent calls and use other Phone Link features.", [column.phoneName])
                        case "needsPhoneAccess":
                            return column.callsState.timedOut === true ? L10n.t("Your phone didn't respond. Unlock it and try again.")
                                 : L10n.f("Recent calls from %@ appear here after you allow access on the phone.", [column.phoneName])
                        default:
                            return L10n.t("Calls from your phone appear here.")
                        }
                    }
                }

                PillButton {
                    Layout.alignment: Qt.AlignHCenter
                    visible: column.content === "turnedOff" || column.content === "needsNotifications"
                             || (column.content === "needsPhoneAccess" && column.callsState.requesting !== true)
                    prominent: true
                    text: column.content === "turnedOff" ? L10n.t("Turn On")
                        : column.content === "needsNotifications" ? L10n.t("Show Me How") : L10n.t("Send Request to Phone")
                    onClicked: {
                        if (column.content === "turnedOff")
                            EngineBridge.perform("calls.turnOn", "")
                        else if (column.content === "needsNotifications")
                            notificationHelp.show()
                        else
                            EngineBridge.perform("calls.requestPermission", "")
                    }
                }

                RowLayout {
                    Layout.alignment: Qt.AlignHCenter
                    visible: column.content === "needsPhoneAccess" && column.callsState.requesting === true
                    spacing: 8

                    BusyIndicator {
                        running: parent.visible
                        padding: 1
                        implicitWidth: 20
                        implicitHeight: 20
                    }

                    Text {
                        text: L10n.t("Check your phone for a permission request...")
                        color: Theme.secondaryText
                        font.pixelSize: Theme.subheadSize
                    }
                }
            }

            ColumnLayout {
                anchors.centerIn: parent
                visible: column.content === "updating"
                spacing: 10

                BusyIndicator {
                    Layout.alignment: Qt.AlignHCenter
                    running: parent.visible
                }

                Text {
                    Layout.alignment: Qt.AlignHCenter
                    text: L10n.t("Updating call history...")
                    color: Theme.secondaryText
                    font.pixelSize: Theme.bodySize
                }
            }
        }
    }
}
