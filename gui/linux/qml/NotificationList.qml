import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import PhoneLink

// The Notifications section's list column: the phone's notifications, pinned ones first, then the rest
// by day, newest first; or what stands in for them while the phone or this computer is not set up for
// them (access on the phone, this computer's switch, nothing yet). Pause and Clear All head the list;
// each row has a menu with the notification's commands and its app's, and turning an app off offers
// Undo. Up and Down move the selection, Delete or Backspace dismisses the selected notification, and
// the Menu key (or Shift+F10) opens its menu.
Rectangle {
    id: column

    property var notifications: []
    property string selectedKey

    // Bumped on the notifications part's `changed` event, so the state below is read again.
    property int revision: 0
    readonly property var status: {
        revision
        EngineBridge.mirroringRevision
        return JSON.parse(EngineBridge.query("notifications.state", "")) || ({ content: "loading" })
    }
    readonly property string content: status.content
    // A list is shown, possibly empty or still syncing: the setup states have no list to pause or clear.
    readonly property bool showsList: content === "list" || content === "empty" || content === "syncing"
    readonly property var rows: groupedRows(notifications)
    // The phone's name; a phone without one is "your phone", and "Your phone" where it starts a sentence.
    readonly property string phoneName: EngineBridge.phoneName.length > 0 ? EngineBridge.phoneName : L10n.t("your phone")
    readonly property string phoneNameAtStart: EngineBridge.phoneName.length > 0 ? EngineBridge.phoneName : L10n.t("Your phone")
    readonly property bool paused: EngineBridge.notificationsMuted
    // The notifications' keys in the engine's order when the list last changed.
    property var listedKeys: []

    // What stands in for the list: its icon, title, text, and the action that sets it up.
    readonly property var setup: {
        switch (content) {
        case "unavailable":
            return { icon: "bellSlash", title: L10n.t("Notifications Aren't Available"),
                     description: L10n.f("%@ doesn't share its notifications with Phone Link.", [phoneNameAtStart]) }
        case "needsPostNotificationPermission":
            return { icon: "bellBadge", title: L10n.t("Allow Link to Windows to Send Notifications"),
                     description: L10n.f("Notifications from %@ appear here after you allow Link to Windows to send notifications on the phone.", [phoneName]),
                     action: L10n.t("Show Me How..."), run: () => permissionHelp.show() }
        case "turnedOff":
            return { icon: "bellSlash", title: L10n.t("Notifications Are Off on This Computer"),
                     description: L10n.f("Turn them on to see the notifications from %@ here.", [phoneName]),
                     action: L10n.t("Turn On"), run: () => EngineBridge.perform("notifications.turnOn", "") }
        case "lowMemoryPhone":
            return { icon: "bellSlash", title: L10n.t("Notifications Aren't Supported"),
                     description: L10n.f("%@ doesn't have enough memory to share its notifications.", [phoneNameAtStart]) }
        case "needsAccess":
            return { icon: "bell", title: L10n.t("Allow Notification Access on Your Phone"),
                     description: L10n.f("Notifications from %@ appear here after you allow access on the phone.", [phoneName]),
                     action: status.requestingAccess ? L10n.t("Waiting for Your Phone...") : L10n.t("Send Request to Phone"),
                     waiting: status.requestingAccess === true,
                     run: () => EngineBridge.perform("notifications.requestAccess", "") }
        case "empty":
            return { icon: "bell", title: L10n.t("No Notifications"),
                     description: L10n.f("New notifications from %@ appear here.", [phoneName]) }
        case "failed":
            return { icon: "warning", title: L10n.t("Couldn't Show Notifications"), description: status.failure || "",
                     action: L10n.t("Try Again"), plain: true, run: () => EngineBridge.perform("notifications.reload", "") }
        default:
            return null
        }
    }

    // Pinned when any notification is pinned, then one section per day, newest day first; a notification
    // dated after today (the phone's clock is ahead) counts as today's. A section header names the day,
    // so a row shows only the time of day, except in Pinned, which mixes days.
    function groupedRows(list) {
        const result = []
        const pinned = list.filter(n => n.pinned)
        if (pinned.length > 0) {
            result.push({ header: L10n.t("Pinned"), id: "pinned" })
            pinned.forEach(n => result.push({ item: n, showsDay: true }))
        }
        const today = Dates.startOfDay(new Date()).getTime()
        const days = []
        const byDay = {}
        list.filter(n => !n.pinned).forEach(n => {
            const day = Math.min(Dates.startOfDay(new Date(n.timeMs)).getTime(), today)
            if (byDay[day] === undefined) {
                byDay[day] = []
                days.push(day)
            }
            byDay[day].push(n)
        })
        days.sort((a, b) => b - a).forEach(day => {
            result.push({ header: Dates.day(day), id: String(day) })
            byDay[day].forEach(n => result.push({ item: n, showsDay: false }))
        })
        return result
    }

    function rowKey(row) {
        return row.header !== undefined ? "header:" + row.id : "item:" + row.item.key
    }

    function notificationOf(key) {
        return notifications.find(n => n.key === key) || null
    }

    function select(key) {
        selectedKey = key
    }

    // Keeps the selection on a listed notification: when the selected one left, the one now at its place
    // is selected, as a deleted mail is followed by the next. While no list shows (the list column shows
    // a setup state, or nothing is left) the selection stays for when the list shows again.
    function keepSelection() {
        const keys = notifications.map(n => n.key)
        if (selectedKey.length > 0 && keys.length > 0 && !keys.includes(selectedKey)) {
            const old = listedKeys.indexOf(selectedKey)
            selectedKey = old >= 0 ? keys[Math.min(old, keys.length - 1)] : ""
        }
        listedKeys = keys
    }

    function rowIndex(key) {
        return rows.findIndex(row => row.item !== undefined && row.item.key === key)
    }

    // Up and Down: the previous or next notification in the order shown.
    function moveSelection(step) {
        const keys = rows.filter(row => row.item !== undefined).map(row => row.item.key)
        if (keys.length === 0)
            return
        const at = keys.indexOf(selectedKey)
        const next = at < 0 ? (step > 0 ? 0 : keys.length - 1) : Math.max(0, Math.min(keys.length - 1, at + step))
        select(keys[next])
        const index = rowIndex(keys[next])
        // The section header above the first notification of a section shows with it.
        if (index > 0 && rows[index - 1].header !== undefined)
            list.positionViewAtIndex(index - 1, ListView.Contain)
        list.positionViewAtIndex(index, ListView.Contain)
    }

    function dismissSelected() {
        const notification = notificationOf(selectedKey)
        if (notification)
            EngineBridge.dismissNotification(notification.key)
    }

    // Selects the notification and puts the keyboard in its reply field.
    function beginReply(key) {
        select(key)
        EngineBridge.perform("notifications.focusReply", key)
    }

    // While the phone screen and its apps open on this computer, a notification the phone can launch
    // opens on the phone, in a mirroring window here. A message's notification opens in Messages
    // instead, and a call's has no open command.
    function opensOnPhone(notification) {
        return status.openOnPhone === true && notification.launchSupported && notification.openTarget === "notifications"
    }

    function appMuted(notification) {
        return notification.appMutedUntilMs !== null && notification.appMutedUntilMs > Date.now()
    }

    // Muting applies to an app that is on, unless every notification is already paused.
    function canMuteApp(notification) {
        const pausedUntil = status.mutedAllUntilMs
        return notification.appOn && (pausedUntil === null || pausedUntil === undefined || pausedUntil <= Date.now())
    }

    function perform(name, request) {
        EngineBridge.perform(name, JSON.stringify(request))
    }

    // A row's commands: the notification's own first, then its app's; Dismiss, destructive, last.
    function menuEntries(notification) {
        const app = notification.app
        const ofApp = { "package": notification["package"], name: app }
        const entries = []
        if (notification.openTarget === "messaging")
            entries.push({ text: L10n.t("Open in Messages"), onTriggered: () => EngineBridge.showSection("messages") })
        if (opensOnPhone(notification))
            entries.push({ text: L10n.t("Open on Phone"), onTriggered: () => EngineBridge.perform("mirror.launchNotification", notification.key) })
        if (notification.replyIndex !== null)
            entries.push({ text: L10n.t("Reply..."), onTriggered: () => column.beginReply(notification.key) })
        notification.actions.forEach(action => entries.push({
            text: action.name, onTriggered: () => EngineBridge.pressNotificationAction(notification.key, action.index)
        }))
        entries.push({ text: notification.pinned ? L10n.t("Unpin") : L10n.t("Pin"),
                       onTriggered: () => EngineBridge.setNotificationPinned(notification.key, !notification.pinned) })

        if (status.appCommands === true) {
            entries.push({ separator: true })
            if (notification.appOn)
                entries.push({ text: L10n.f("Turn Off Notifications from %@", [app]), onTriggered: () => perform("notifications.turnOffApp", ofApp) })
            else
                entries.push({ text: L10n.f("Turn On Notifications from %@", [app]), onTriggered: () => perform("notifications.turnOnApp", ofApp) })
            if (canMuteApp(notification)) {
                if (appMuted(notification)) {
                    entries.push({ text: L10n.f("Unmute %@", [app]), onTriggered: () => perform("notifications.unmuteApp", ofApp) })
                } else {
                    const mute = duration => () => perform("notifications.muteApp", Object.assign({ duration: duration }, ofApp))
                    entries.push({ text: L10n.f("Mute %@", [app]), submenu: [
                        { text: L10n.t("For 1 Hour"), onTriggered: mute("oneHour") },
                        { text: L10n.t("For 8 Hours"), onTriggered: mute("eightHours") },
                        { text: L10n.t("For 24 Hours"), onTriggered: mute("oneDay") }
                    ] })
                }
            }
        }

        entries.push({ separator: true })
        entries.push({ text: L10n.t("Dismiss"), destructive: true, onTriggered: () => EngineBridge.dismissNotification(notification.key) })
        return entries
    }

    // The row's menu at the pointer, or under the row `item` (from the keyboard).
    function showMenu(notification, item) {
        rowMenu.show(menuEntries(notification), item, 24, item ? item.height - 6 : 0)
    }

    function showMenuForSelected() {
        const notification = notificationOf(selectedKey)
        if (notification === null)
            return
        const index = rowIndex(notification.key)
        list.positionViewAtIndex(index, ListView.Contain)
        const loader = list.itemAtIndex(index)
        showMenu(notification, loader ? loader.item : null)
    }

    function pauseHelp() {
        if (!paused)
            return L10n.t("Pause all notifications for 24 hours")
        const until = status.mutedAllUntilMs
        if (until === null || until === undefined)
            return L10n.t("Resume Notifications")
        return L10n.f("Resume Notifications — %@", [L10n.f("Paused until %@", [texts.until(until)])])
    }

    function undoTurnOff() {
        if (undoNotice.app === null)
            return
        EngineBridge.perform("notifications.undoTurnOff", undoNotice.app["package"])
        undoNotice.app = null
    }

    color: Theme.listBackground
    onRowsChanged: rowModel.sync(rows, rowKey)
    onNotificationsChanged: keepSelection()
    onContentChanged: {
        // The help belongs to the post-notification stage; it closes once the phone reports the permission.
        if (content !== "needsPostNotificationPermission" && permissionHelp.visible)
            permissionHelp.close()
    }
    Component.onCompleted: {
        rowModel.sync(rows, rowKey)
        listedKeys = notifications.map(n => n.key)
    }

    Connections {
        target: EngineBridge

        function onAreaEvent(area, name, payload) {
            if (area !== "notifications")
                return
            if (name === "changed") {
                column.revision++
            } else if (name === "appTurnedOff") {
                undoNotice.app = JSON.parse(payload)
                undoTimer.restart()
            }
        }

        // A click on a desktop notification selects it (MainView shows the section).
        function onShowNotification(key) {
            column.select(key)
        }
    }

    NotificationText {
        id: texts
    }

    KeyedListModel {
        id: rowModel
    }

    // The engine announces no end of a pause: the state is read again a moment after it ends.
    Timer {
        readonly property var until: column.status.mutedAllUntilMs
        running: until !== null && until !== undefined
        interval: running ? Math.max(1000, until - Date.now() + 1000) : 1000
        onTriggered: EngineBridge.perform("notifications.reload", "")
    }

    Shortcut {
        sequences: [StandardKey.Undo]
        enabled: undoNotice.visible && column.visible
        onActivated: column.undoTurnOff()
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
                text: L10n.t("Notifications")
                color: Theme.text
                font.pixelSize: Theme.headlineSize
                font.weight: Font.Bold
                elide: Text.ElideRight
            }

            // Pauses every desktop notification from the phone for 24 hours, or ends the pause; it applies
            // only while the phone's notifications show here.
            IconButton {
                visible: column.showsList
                enabled: column.status.pcPermission === true
                iconName: "bellSlash"
                label: column.paused ? L10n.t("Resume Notifications") : L10n.t("Pause Notifications")
                active: column.paused
                ToolTip.text: column.pauseHelp()
                onClicked: EngineBridge.muteNotifications(!column.paused)
            }

            PillButton {
                visible: column.showsList
                text: L10n.t("Clear All")
                enabled: column.status.canClearAll === true
                onClicked: clearConfirmation.open()
            }
        }

        Item {
            Layout.fillWidth: true
            Layout.fillHeight: true

            BusyIndicator {
                anchors.centerIn: parent
                visible: column.content === "loading"
                running: visible
            }

            ColumnLayout {
                anchors.centerIn: parent
                width: parent.width - 40
                visible: column.content === "syncing"
                spacing: 12

                BusyIndicator {
                    Layout.alignment: Qt.AlignHCenter
                    running: parent.visible
                }

                Text {
                    Layout.fillWidth: true
                    text: L10n.t("Getting Notifications...")
                    color: Theme.secondaryText
                    font.pixelSize: Theme.bodySize
                    horizontalAlignment: Text.AlignHCenter
                    wrapMode: Text.Wrap
                }
            }

            NotificationSetupState {
                anchors.centerIn: parent
                width: parent.width - 40
                visible: column.setup !== null
                iconName: column.setup ? column.setup.icon : ""
                title: column.setup ? column.setup.title : ""
                description: column.setup ? column.setup.description : ""
                actionText: column.setup && column.setup.action ? column.setup.action : ""
                actionEnabled: !(column.setup && column.setup.waiting)
                prominent: !(column.setup && column.setup.plain)
                onActionTriggered: column.setup.run()
            }

            ListView {
                id: list

                anchors.fill: parent
                visible: column.content === "list"
                clip: true
                model: rowModel
                leftMargin: 10
                rightMargin: 10
                bottomMargin: undoNotice.visible ? undoNotice.height + 20 : 10
                activeFocusOnTab: true
                ScrollBar.vertical: ScrollBar {}
                Accessible.role: Accessible.List
                Accessible.name: L10n.t("Notifications")

                Keys.onUpPressed: column.moveSelection(-1)
                Keys.onDownPressed: column.moveSelection(1)
                Keys.onDeletePressed: column.dismissSelected()
                Keys.onPressed: event => {
                    if (event.key === Qt.Key_Backspace) {
                        column.dismissSelected()
                        event.accepted = true
                    } else if (event.key === Qt.Key_Menu || (event.key === Qt.Key_F10 && (event.modifiers & Qt.ShiftModifier))) {
                        column.showMenuForSelected()
                        event.accepted = true
                    }
                }

                delegate: Loader {
                    required property string json
                    readonly property var modelData: JSON.parse(json)
                    width: ListView.view.width - 20
                    sourceComponent: modelData.header !== undefined ? headerRow : itemRow

                    Component {
                        id: headerRow

                        Item {
                            implicitHeight: 34

                            Text {
                                anchors.left: parent.left
                                anchors.leftMargin: 6
                                anchors.right: parent.right
                                anchors.bottom: parent.bottom
                                anchors.bottomMargin: 6
                                text: modelData.header
                                color: Theme.secondaryText
                                font.pixelSize: Theme.subheadSize
                                font.weight: Font.DemiBold
                                elide: Text.ElideRight
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
                        id: itemRow

                        AbstractButton {
                            id: row

                            readonly property var notification: modelData.item
                            readonly property bool selected: column.selectedKey === notification.key
                            // The time of day where the section names the day; in Pinned, the day too.
                            readonly property string time: modelData.showsDay ? Dates.listStamp(notification.timeMs)
                                                                              : Dates.time(notification.timeMs)

                            implicitHeight: content.implicitHeight + 16
                            hoverEnabled: true
                            focusPolicy: Qt.NoFocus
                            Accessible.role: Accessible.Button
                            Accessible.name: [notification.app, time, notification.title, notification.preview]
                                .filter(part => part.length > 0).join(", ")
                            onClicked: {
                                column.select(notification.key)
                                list.forceActiveFocus()
                            }

                            TapHandler {
                                acceptedButtons: Qt.RightButton
                                onTapped: column.showMenu(row.notification, null)
                            }

                            background: Rectangle {
                                radius: Theme.rowRadius
                                color: row.selected ? Theme.selection : row.hovered ? Theme.hover : "transparent"
                                border.width: row.selected && list.activeFocus ? 1 : 0
                                border.color: Theme.accent
                            }

                            contentItem: Item {
                                ColumnLayout {
                                    id: content
                                    anchors.left: parent.left
                                    anchors.right: parent.right
                                    anchors.verticalCenter: parent.verticalCenter
                                    anchors.leftMargin: 6
                                    anchors.rightMargin: 6
                                    spacing: 2

                                    RowLayout {
                                        Layout.fillWidth: true
                                        spacing: 6

                                        AppGlyph {
                                            name: row.notification.app
                                            packageName: row.notification["package"]
                                            size: 18
                                        }

                                        Text {
                                            Layout.fillWidth: true
                                            text: row.notification.app
                                            color: Theme.secondaryText
                                            font.pixelSize: Theme.subheadSize
                                            elide: Text.ElideRight
                                        }

                                        Text {
                                            text: row.time
                                            color: Theme.secondaryText
                                            font.pixelSize: Theme.captionSize
                                        }
                                    }

                                    Text {
                                        Layout.fillWidth: true
                                        visible: text.length > 0
                                        text: row.notification.title
                                        color: Theme.text
                                        font.pixelSize: Theme.bodySize + 1
                                        font.weight: Font.Bold
                                        elide: Text.ElideRight
                                    }

                                    Text {
                                        Layout.fillWidth: true
                                        visible: text.length > 0
                                        text: row.notification.preview
                                        color: Theme.secondaryText
                                        font.pixelSize: Theme.subheadSize
                                        wrapMode: Text.Wrap
                                        maximumLineCount: 2
                                        elide: Text.ElideRight
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // Undo for an app turned off from a row's menu (also Ctrl+Z while it shows).
            Rectangle {
                id: undoNotice

                property var app: null

                anchors.left: parent.left
                anchors.right: parent.right
                anchors.bottom: parent.bottom
                anchors.margins: 10
                implicitHeight: undoRow.implicitHeight + 16
                visible: app !== null && column.showsList
                radius: Theme.moduleRadius
                color: Theme.raisedBackground
                border.color: Theme.separator
                Accessible.role: Accessible.AlertMessage
                Accessible.name: undoText.text

                Timer {
                    id: undoTimer
                    interval: 10000
                    running: undoNotice.visible && !undoHover.hovered
                    onTriggered: undoNotice.app = null
                }

                HoverHandler {
                    id: undoHover
                }

                RowLayout {
                    id: undoRow
                    anchors.fill: parent
                    anchors.leftMargin: 12
                    anchors.rightMargin: 6
                    spacing: 8

                    Text {
                        id: undoText
                        Layout.fillWidth: true
                        text: undoNotice.app ? L10n.f("Turned off notifications from %@.", [undoNotice.app.name]) : ""
                        color: Theme.text
                        font.pixelSize: Theme.subheadSize
                        wrapMode: Text.Wrap
                    }

                    PillButton {
                        text: L10n.t("Undo")
                        onClicked: column.undoTurnOff()
                    }

                    IconButton {
                        implicitWidth: 24
                        implicitHeight: 24
                        iconName: "close"
                        label: L10n.t("Close")
                        onClicked: undoNotice.app = null
                    }
                }
            }
        }
    }

    NotificationMenu {
        id: rowMenu
    }

    NotificationPermissionHelp {
        id: permissionHelp
        phoneName: column.phoneName
    }

    // The phone clears the notifications too, for good.
    MessageDialog {
        id: clearConfirmation
        heading: L10n.t("Clear All Notifications?")
        message: L10n.f("Unpinned notifications are also cleared on %@. This can't be undone.", [column.phoneName])
        acceptText: L10n.t("Clear All")
        destructive: true
        onAccepted: EngineBridge.perform("notifications.clearAll", "")
    }
}
