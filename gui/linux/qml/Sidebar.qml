import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import Qt5Compat.GraphicalEffects
import PhoneLink

// The sidebar: the phone (its status and controls) on top whatever section is selected, the sections
// below, and at the bottom what is happening now (a transfer, what plays on the phone). The phone's
// wallpaper shows behind the sidebar while the user keeps it on. Files dropped on the phone area go to
// the phone.
Rectangle {
    id: sidebar

    property string section: "messages"
    property var badges: ({})

    signal settingsRequested()

    readonly property var status: JSON.parse(EngineBridge.statusJson)
    readonly property var settings: { EngineBridge.settingsRevision; return JSON.parse(EngineBridge.query("settings", "")) || {} }
    readonly property var connection: {
        EngineBridge.settingsRevision
        EngineBridge.phoneConnected
        return JSON.parse(EngineBridge.query("shell.connection", "")) || {}
    }
    readonly property string wallpaper: { EngineBridge.wallpaperRevision; return JSON.parse(EngineBridge.query("wallpaper", "")) || "" }
    readonly property var mirroring: { EngineBridge.mirroringRevision; return JSON.parse(EngineBridge.query("mirror.status", "")) }
    readonly property bool mirroringOffered: mirroring !== null && mirroring.offered === true
    readonly property var camera: { EngineBridge.settingsRevision; return JSON.parse(EngineBridge.query("camera", "")) }
    readonly property var phoneFiles: { EngineBridge.settingsRevision; return JSON.parse(EngineBridge.query("phoneFiles", "")) }
    readonly property var phones: {
        EngineBridge.phonesJson
        EngineBridge.pairingRevision
        return JSON.parse(EngineBridge.query("shell.phones", "")) || []
    }
    // The sections the engine says the phone does not offer are not listed.
    readonly property var offered: {
        EngineBridge.callsRevision
        EngineBridge.contactsRevision
        EngineBridge.appsRevision
        return JSON.parse(EngineBridge.query("shell.sections", "")) || {}
    }
    readonly property bool live: status.live === true
    readonly property bool connected: EngineBridge.phoneConnected
    readonly property bool phoneKnown: EngineBridge.phoneName.length > 0
    readonly property var sections: [
        { id: "notifications", title: L10n.t("Notifications"), icon: "bell" },
        { id: "messages", title: L10n.t("Messages"), icon: "message" },
        { id: "calls", title: L10n.t("Calls"), icon: "phone" },
        { id: "contacts", title: L10n.t("Contacts"), icon: "person" },
        { id: "photos", title: L10n.t("Photos"), icon: "photo" },
        { id: "apps", title: L10n.t("Apps"), icon: "apps" }
    ].filter(section => offered[section.id] !== false)

    // A control's tile while the phone is connected; its name only, and not clickable, while it is not.
    function tileOf(control) {
        if (!control || !control.tile || !connected)
            return { state: null, on: false, busy: false, enabled: false }
        return control.tile
    }

    function ringerIcon(mode) {
        switch (mode) {
        case "vibrate": return "vibrate"
        case "silent": return "bellSlash"
        default: return "bell"
        }
    }

    // A connect or disconnect runs while busy; this computer is still on the hotspot while it leaves.
    function hotspotSubtitle() {
        if (settings.hotspotBusy === true)
            return settings.hotspotJoined === true ? L10n.t("Disconnecting...") : L10n.t("Connecting...")
        return settings.hotspotJoined === true ? L10n.t("Connected") : ""
    }

    function cameraSubtitle() {
        if (camera === null || camera.inUse !== true)
            return ""
        switch (camera.state) {
        case "Streaming": return L10n.t("Streaming")
        case "Paused": return L10n.t("Paused")
        case "StartStreamNotification": return L10n.t("Check your phone")
        case "Error":
        case "TerminalError": return L10n.t("Stopped")
        default: return L10n.t("Connecting...")
        }
    }

    color: Theme.sidebarBackground
    clip: true

    // A section the phone stops offering is left for Messages.
    onSectionsChanged: {
        if (!sections.some(item => item.id === section))
            section = "messages"
    }

    Image {
        id: wallpaperImage
        anchors.fill: parent
        visible: false
        source: Preferences.wallpaperBackground ? sidebar.wallpaper : ""
        fillMode: Image.PreserveAspectCrop
    }

    FastBlur {
        anchors.fill: parent
        visible: wallpaperImage.status === Image.Ready
        source: wallpaperImage
        radius: 64
    }

    Rectangle {
        anchors.fill: parent
        visible: wallpaperImage.status === Image.Ready
        color: Theme.sidebarBackground
        opacity: 0.72
    }

    Menu {
        id: phoneMenu

        MenuItem {
            id: phoneFilesItem
            readonly property bool shown: sidebar.phoneFiles !== null && sidebar.phoneFiles.running === true
                && sidebar.phoneFiles.permission === true
            visible: shown
            height: shown ? implicitHeight : 0
            text: L10n.t("Open Phone Files")
            onTriggered: EngineBridge.perform("phoneFiles.open", "")
        }

        // The phone's free and total storage, as a line under Open Phone Files.
        MenuItem {
            readonly property string storage: phoneFilesItem.shown && typeof sidebar.phoneFiles.storage === "string"
                ? sidebar.phoneFiles.storage : ""
            visible: storage.length > 0
            height: visible ? implicitHeight : 0
            enabled: false
            text: storage
        }

        MenuSeparator {
            visible: phoneFilesItem.shown
            height: visible ? implicitHeight : 0
        }

        Instantiator {
            model: sidebar.phones.length > 1 ? sidebar.phones : []
            delegate: MenuItem {
                required property var modelData
                checkable: true
                checked: modelData.inUse === true
                text: modelData.name
                onTriggered: {
                    checked = Qt.binding(() => modelData.inUse === true)
                    if (modelData.inUse !== true)
                        EngineBridge.selectPhone(modelData.id)
                }
            }
            // After the phone's files, their storage line and the separator.
            onObjectAdded: (index, object) => phoneMenu.insertItem(index + 3, object)
            onObjectRemoved: (index, object) => phoneMenu.removeItem(object)
        }

        MenuSeparator {
            visible: sidebar.phones.length > 1
            height: visible ? implicitHeight : 0
        }

        MenuItem {
            text: L10n.t("Pair a New Phone...")
            onTriggered: EngineBridge.perform("shell.showPairing", "")
        }
    }

    Flickable {
        id: scroller
        anchors.fill: parent
        contentWidth: width
        contentHeight: Math.max(height, column.implicitHeight + 2 * Theme.edge)
        boundsBehavior: Flickable.StopAtBounds
        clip: true
        ScrollBar.vertical: ScrollBar {
            policy: scroller.contentHeight > scroller.height ? ScrollBar.AsNeeded : ScrollBar.AlwaysOff
        }

        // The cards stay at the bottom while everything fits; a short window scrolls the sidebar.
        ColumnLayout {
            id: column
            x: Theme.edge
            y: Theme.edge
            width: scroller.width - 2 * Theme.edge
            height: scroller.contentHeight - 2 * Theme.edge
            spacing: 8

            // The phone area: its picture and name, its status, why it is not connected, its controls.
            Item {
                Layout.fillWidth: true
                implicitHeight: phoneColumn.implicitHeight

                ColumnLayout {
                    id: phoneColumn
                    anchors.left: parent.left
                    anchors.right: parent.right
                    spacing: 8

                    RowLayout {
                        Layout.fillWidth: true
                        Layout.leftMargin: Theme.content - Theme.edge
                        Layout.topMargin: 6
                        spacing: 10

                        Rectangle {
                            Layout.preferredWidth: 28
                            Layout.preferredHeight: 48
                            radius: 6
                            clip: true
                            border.color: Theme.separator
                            gradient: Gradient {
                                GradientStop { position: 0; color: "#8fb8de" }
                                GradientStop { position: 1; color: "#7fae7a" }
                            }
                            Accessible.role: Accessible.Button
                            Accessible.name: L10n.t("Open Phone Screen")

                            Image {
                                anchors.fill: parent
                                anchors.margins: 1
                                source: sidebar.wallpaper
                                fillMode: Image.PreserveAspectCrop
                                sourceSize.width: 56
                            }

                            MouseArea {
                                anchors.fill: parent
                                enabled: sidebar.mirroringOffered
                                hoverEnabled: true
                                cursorShape: enabled ? Qt.PointingHandCursor : Qt.ArrowCursor
                                onClicked: EngineBridge.perform("mirror.openPhoneScreen", "")
                                ToolTip.visible: containsMouse
                                ToolTip.delay: 600
                                ToolTip.text: L10n.t("Open Phone Screen")
                            }
                        }

                        ColumnLayout {
                            Layout.fillWidth: true
                            spacing: 4

                            // The phone's name opens a menu: its files, the account's other phones,
                            // pairing one.
                            AbstractButton {
                                id: nameButton
                                Layout.fillWidth: true
                                Layout.maximumWidth: implicitWidth
                                hoverEnabled: true
                                Accessible.role: Accessible.ButtonMenu
                                Accessible.name: nameText.text
                                onClicked: phoneMenu.popup(nameButton, 0, nameButton.height)

                                contentItem: RowLayout {
                                    spacing: 4

                                    Text {
                                        id: nameText
                                        Layout.fillWidth: true
                                        text: sidebar.phoneKnown ? EngineBridge.phoneName : L10n.t("Phone")
                                        color: Theme.text
                                        font.pixelSize: Theme.headlineSize + 2
                                        font.weight: Font.DemiBold
                                        elide: Text.ElideRight
                                    }

                                    Icon {
                                        name: "chevronDown"
                                        size: 12
                                        color: Theme.secondaryText
                                    }
                                }

                                background: Rectangle {
                                    anchors.fill: parent
                                    anchors.margins: -2
                                    radius: Theme.rowRadius
                                    color: nameButton.hovered || nameButton.visualFocus ? Theme.hover : "transparent"
                                }
                            }

                            // The phone's status bar: Wi-Fi, cellular, then the battery.
                            RowLayout {
                                spacing: 8
                                visible: sidebar.connected && sidebar.live && !EngineBridge.featuresDisabled

                                Icon {
                                    visible: sidebar.status.wifiBars !== null && sidebar.status.wifiBars !== undefined
                                    name: "wifi"
                                    size: 13
                                    color: Theme.secondaryText
                                }

                                Row {
                                    visible: sidebar.status.cellBars !== null && sidebar.status.cellBars !== undefined
                                    spacing: 1.5
                                    Repeater {
                                        model: sidebar.status.cellMax || 4
                                        Rectangle {
                                            required property int index
                                            width: 2.5
                                            height: 4 + index * 2
                                            anchors.bottom: parent.bottom
                                            radius: 1
                                            color: Theme.secondaryText
                                            opacity: index < sidebar.status.cellBars ? 1 : 0.3
                                        }
                                    }
                                }

                                Text {
                                    visible: sidebar.status.battery !== null && sidebar.status.battery !== undefined
                                    text: sidebar.status.battery + "%"
                                    color: Theme.secondaryText
                                    font.pixelSize: Theme.subheadSize
                                }

                                Rectangle {
                                    visible: sidebar.status.battery !== null && sidebar.status.battery !== undefined
                                    width: 22
                                    height: 11
                                    radius: 3
                                    color: "transparent"
                                    border.color: Theme.secondaryText

                                    Rectangle {
                                        x: 2
                                        y: 2
                                        height: parent.height - 4
                                        width: Math.max(1, (parent.width - 4) * (sidebar.status.battery || 0) / 100)
                                        radius: 1.5
                                        color: sidebar.status.charging ? "#30d158" : Theme.secondaryText
                                    }
                                }
                            }

                            // What is happening instead while the phone is not connected.
                            RowLayout {
                                Layout.fillWidth: true
                                visible: !(sidebar.connected && sidebar.live && !EngineBridge.featuresDisabled)
                                spacing: 5

                                readonly property bool connecting: !EngineBridge.featuresDisabled
                                    && (!sidebar.phoneKnown || sidebar.connected)

                                BusyIndicator {
                                    Layout.preferredWidth: 14
                                    Layout.preferredHeight: 14
                                    padding: 0
                                    visible: parent.connecting
                                    running: visible
                                }

                                Icon {
                                    visible: !EngineBridge.featuresDisabled && !parent.connecting
                                    name: "warning"
                                    size: 13
                                    color: Theme.warning
                                }

                                Text {
                                    Layout.fillWidth: true
                                    text: EngineBridge.featuresDisabled ? L10n.t("Link to Windows is turned off")
                                        : parent.connecting ? L10n.t("Connecting...") : L10n.t("Not connected")
                                    color: Theme.secondaryText
                                    font.pixelSize: Theme.subheadSize
                                    elide: Text.ElideRight
                                }
                            }
                        }

                        Item {
                            Layout.fillWidth: true
                        }

                        IconButton {
                            Layout.alignment: Qt.AlignTop
                            iconName: "gear"
                            label: L10n.t("Settings")
                            onClicked: sidebar.settingsRequested()
                        }
                    }

                    // While the phone can't be reached: why, and Reconnect. The line above already says it
                    // is not connected.
                    Rectangle {
                        Layout.fillWidth: true
                        Layout.topMargin: 4
                        visible: sidebar.phoneKnown && !sidebar.connected && !EngineBridge.featuresDisabled
                        implicitHeight: noticeRow.implicitHeight + 16
                        radius: Theme.moduleRadius
                        color: Theme.sidebarModule

                        RowLayout {
                            id: noticeRow
                            anchors.fill: parent
                            anchors.leftMargin: Theme.content - Theme.edge
                            anchors.rightMargin: 8
                            anchors.topMargin: 8
                            anchors.bottomMargin: 8
                            spacing: 8

                            Text {
                                Layout.fillWidth: true
                                text: sidebar.connection.failure || L10n.t("Your phone can't be reached right now.")
                                color: Theme.secondaryText
                                font.pixelSize: Theme.subheadSize
                                wrapMode: Text.Wrap
                            }

                            PillButton {
                                text: L10n.t("Reconnect")
                                onClicked: EngineBridge.perform("app.refresh", "")
                            }
                        }
                    }

                    // The phone's controls, two per row, each only while the phone offers it.
                    GridLayout {
                        Layout.fillWidth: true
                        Layout.topMargin: 6
                        columns: 2
                        columnSpacing: 6
                        rowSpacing: 6

                        ControlTile {
                            readonly property var tile: sidebar.tileOf(sidebar.status.dnd)
                            Layout.fillWidth: true
                            Layout.preferredWidth: 1
                            visible: !!sidebar.status.dnd
                            enabled: tile.enabled
                            busy: tile.busy
                            on: tile.on
                            iconName: "moon"
                            title: L10n.t("Do Not Disturb")
                            subtitle: tile.state || ""
                            onClicked: EngineBridge.toggleDoNotDisturb()
                        }

                        ControlTile {
                            readonly property var tile: sidebar.tileOf(sidebar.status.ringer)
                            Layout.fillWidth: true
                            Layout.preferredWidth: 1
                            visible: !!sidebar.status.ringer
                            enabled: tile.enabled
                            busy: tile.busy
                            on: tile.on
                            iconName: sidebar.ringerIcon(sidebar.status.ringer ? sidebar.status.ringer.mode : "")
                            title: L10n.t("Ringer")
                            subtitle: tile.state || ""
                            onClicked: EngineBridge.cycleRinger()
                        }

                        ControlTile {
                            Layout.fillWidth: true
                            Layout.preferredWidth: 1
                            visible: sidebar.settings.hotspotSupported === true
                            enabled: sidebar.connected && sidebar.settings.hotspotBusy !== true
                            busy: sidebar.connected && sidebar.settings.hotspotBusy === true
                            on: sidebar.connected && sidebar.settings.hotspotJoined === true && sidebar.settings.hotspotBusy !== true
                            iconName: "hotspot"
                            title: L10n.t("Hotspot")
                            subtitle: sidebar.connected ? sidebar.hotspotSubtitle() : ""
                            onClicked: EngineBridge.perform(sidebar.settings.hotspotJoined === true ? "hotspot.disconnect" : "hotspot.connect", "")
                        }

                        ControlTile {
                            readonly property var tile: sidebar.tileOf(sidebar.status.ring)
                            Layout.fillWidth: true
                            Layout.preferredWidth: 1
                            visible: !!sidebar.status.ring
                            enabled: tile.enabled
                            busy: tile.busy
                            on: tile.on
                            iconName: tile.on ? "ringPhone" : "findPhone"
                            title: L10n.t("Find Phone")
                            subtitle: tile.state || ""
                            onClicked: EngineBridge.findPhone()
                        }

                        ControlTile {
                            Layout.fillWidth: true
                            Layout.preferredWidth: 1
                            visible: sidebar.settings.canSendFiles === true
                            enabled: sidebar.connected
                            iconName: "upload"
                            title: L10n.t("Send Files")
                            onClicked: EngineBridge.perform("files.showSendWindow", "")
                        }

                        ControlTile {
                            Layout.fillWidth: true
                            Layout.preferredWidth: 1
                            visible: sidebar.mirroringOffered
                            enabled: sidebar.connected
                            iconName: "phoneScreen"
                            title: L10n.t("Phone Screen")
                            onClicked: EngineBridge.perform("mirror.openPhoneScreen", "")
                        }

                        ControlTile {
                            Layout.fillWidth: true
                            Layout.preferredWidth: 1
                            visible: sidebar.camera !== null && sidebar.camera.enabled === true && sidebar.camera.registered === true
                            enabled: sidebar.connected
                            iconName: "device"
                            title: L10n.t("Camera")
                            subtitle: sidebar.connected ? sidebar.cameraSubtitle() : ""
                            on: sidebar.connected && sidebar.camera !== null && sidebar.camera.inUse === true
                            onClicked: EngineBridge.perform("camera.use", String(!on))
                        }
                    }
                }

                // Files dropped on the phone area go to the phone from the Send Files window, which shows
                // the progress and the outcome.
                DropArea {
                    id: drop
                    anchors.fill: parent
                    enabled: sidebar.settings.canSendFiles === true
                    onEntered: drag => drag.accept(Qt.CopyAction)
                    onDropped: drop => {
                        const urls = drop.urls.map(url => url.toString()).filter(url => url.startsWith("file:"))
                        if (urls.length > 0)
                            EngineBridge.perform("files.showSendWindow", JSON.stringify(urls))
                    }

                    Rectangle {
                        anchors.fill: parent
                        anchors.margins: -Theme.edge / 2
                        visible: drop.containsDrag
                        radius: Theme.moduleRadius
                        color: "transparent"
                        border.color: Theme.accent
                        border.width: 2
                    }
                }
            }

            Rectangle {
                Layout.fillWidth: true
                Layout.topMargin: 4
                Layout.bottomMargin: 4
                implicitHeight: 1
                color: Theme.separator
            }

            Repeater {
                model: sidebar.sections

                AbstractButton {
                    id: item
                    required property var modelData
                    Layout.fillWidth: true
                    implicitHeight: 32
                    hoverEnabled: true
                    Accessible.role: Accessible.Button
                    Accessible.name: modelData.title
                    onClicked: sidebar.section = modelData.id

                    background: Rectangle {
                        radius: Theme.rowRadius
                        color: sidebar.section === item.modelData.id ? Theme.selection
                             : item.hovered ? Theme.hover : "transparent"
                    }

                    contentItem: RowLayout {
                        spacing: 8

                        Icon {
                            Layout.leftMargin: 8
                            name: item.modelData.icon
                            size: 18
                            color: Theme.accent
                        }

                        Text {
                            Layout.fillWidth: true
                            text: item.modelData.title
                            color: Theme.text
                            font.pixelSize: Theme.bodySize + 1
                            font.weight: sidebar.section === item.modelData.id ? Font.DemiBold : Font.Normal
                        }

                        Text {
                            Layout.rightMargin: 8
                            visible: (sidebar.badges[item.modelData.id] || 0) > 0
                            text: sidebar.badges[item.modelData.id] || ""
                            color: Theme.secondaryText
                            font.pixelSize: Theme.captionSize
                        }
                    }
                }
            }

            Item {
                Layout.fillHeight: true
            }

            TransferCard {
                Layout.fillWidth: true
            }

            MediaCard {
                Layout.fillWidth: true
            }
        }
    }
}
