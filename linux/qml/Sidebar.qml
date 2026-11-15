import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Dialogs
import QtQuick.Effects
import QtQuick.Layouts
import PhoneLink

Rectangle {
    id: sidebar

    property string section: "notifications"
    property var badges: ({})

    signal settingsRequested()

    readonly property var status: JSON.parse(EngineBridge.statusJson)
    readonly property var settings: { EngineBridge.settingsRevision; return JSON.parse(EngineBridge.query("settings", "")) || {} }
    readonly property string wallpaper: { EngineBridge.wallpaperRevision; return JSON.parse(EngineBridge.query("wallpaper", "")) || "" }
    readonly property var mirroring: { EngineBridge.mirroringRevision; return JSON.parse(EngineBridge.query("mirror.status", "")) }
    readonly property bool mirroringOffered: mirroring !== null && mirroring.supported && mirroring.phoneScreen
        && mirroring.apps !== "Disabled"
    readonly property var sections: [
        { id: "notifications", title: L10n.t("Notifications"), icon: "bell" },
        { id: "messages", title: L10n.t("Messages"), icon: "message" },
        { id: "calls", title: L10n.t("Calls"), icon: "phone" },
        { id: "contacts", title: L10n.t("Contacts"), icon: "person" },
        { id: "photos", title: L10n.t("Photos"), icon: "photo" },
        { id: "apps", title: L10n.t("Apps"), icon: "apps" }
    ]

    function dndSubtitle(dnd) {
        switch (dnd.shown) {
        case "on": return L10n.t("On")
        case "off": return L10n.t("Off")
        case "turningOn": return L10n.t("Turning On...")
        case "turningOff": return L10n.t("Turning Off...")
        default: return ""
        }
    }

    function ringerSubtitle(ringer) {
        switch (ringer.shown) {
        case "silent": return L10n.t("Silent")
        case "vibrate": return L10n.t("Vibrate")
        case "sound": return L10n.t("Sound")
        case "changing": return L10n.t("Changing...")
        default: return ""
        }
    }

    color: Theme.sidebarBackground
    clip: true

    Image {
        id: wallpaperImage
        anchors.fill: parent
        visible: false
        source: Preferences.wallpaperBackground ? sidebar.wallpaper : ""
        fillMode: Image.PreserveAspectCrop
    }

    MultiEffect {
        anchors.fill: parent
        visible: wallpaperImage.status === Image.Ready
        source: wallpaperImage
        blurEnabled: true
        blur: 1
        blurMax: 64
    }

    Rectangle {
        anchors.fill: parent
        visible: wallpaperImage.status === Image.Ready
        color: Theme.sidebarBackground
        opacity: 0.72
    }

    FileDialog {
        id: sendDialog
        title: L10n.t("Send Files")
        fileMode: FileDialog.OpenFiles
        onAccepted: EngineBridge.perform("files.send", JSON.stringify(selectedFiles.map(url => url.toString())))
    }

    ColumnLayout {
        anchors.fill: parent
        anchors.margins: Theme.edge
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

                Image {
                    anchors.fill: parent
                    anchors.margins: 1
                    source: sidebar.wallpaper
                    fillMode: Image.PreserveAspectCrop
                    sourceSize.width: 56
                }

                MouseArea {
                    id: phoneScreenButton
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

                Text {
                    Layout.fillWidth: true
                    text: EngineBridge.phoneName.length > 0 ? EngineBridge.phoneName : L10n.t("Phone")
                    color: Theme.text
                    font.pixelSize: Theme.headlineSize + 2
                    font.weight: Font.DemiBold
                    elide: Text.ElideRight
                }

                RowLayout {
                    spacing: 8
                    visible: sidebar.status.live === true

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

                Text {
                    visible: sidebar.status.live !== true
                    text: EngineBridge.phoneConnected ? L10n.t("Connecting...") : L10n.t("Not connected")
                    color: EngineBridge.phoneConnected ? Theme.warning : Theme.secondaryText
                    font.pixelSize: Theme.subheadSize
                }
            }

            IconButton {
                Layout.alignment: Qt.AlignTop
                iconName: "gear"
                onClicked: sidebar.settingsRequested()
            }
        }

        GridLayout {
            Layout.fillWidth: true
            Layout.topMargin: 6
            columns: 2
            columnSpacing: 6
            rowSpacing: 6

            ControlTile {
                Layout.fillWidth: true
                Layout.preferredWidth: 1
                visible: !!sidebar.status.dnd
                enabled: !!sidebar.status.dnd && sidebar.status.dnd.enabled
                iconName: "moon"
                title: L10n.t("Do Not Disturb")
                subtitle: sidebar.status.dnd ? sidebar.dndSubtitle(sidebar.status.dnd) : ""
                on: !!sidebar.status.dnd && sidebar.status.dnd.shown === "on"
                onClicked: EngineBridge.toggleDoNotDisturb()
            }

            ControlTile {
                Layout.fillWidth: true
                Layout.preferredWidth: 1
                visible: !!sidebar.status.ringer
                enabled: !!sidebar.status.ringer && sidebar.status.ringer.enabled
                iconName: sidebar.status.ringer && sidebar.status.ringer.shown === "vibrate" ? "vibrate"
                        : sidebar.status.ringer && sidebar.status.ringer.shown === "silent" ? "speakerSlash" : "bell"
                title: L10n.t("Ringer")
                subtitle: sidebar.status.ringer ? sidebar.ringerSubtitle(sidebar.status.ringer) : ""
                onClicked: EngineBridge.cycleRinger()
            }

            ControlTile {
                Layout.fillWidth: true
                Layout.preferredWidth: 1
                visible: !!sidebar.status.ring
                enabled: !!sidebar.status.ring && (sidebar.status.ring.enabled || sidebar.status.ring.canStop)
                iconName: "findPhone"
                title: L10n.t("Find Phone")
                subtitle: sidebar.status.ring && sidebar.status.ring.ringing ? L10n.t("Ringing") : ""
                on: !!sidebar.status.ring && sidebar.status.ring.ringing
                onClicked: EngineBridge.findPhone()
            }

            ControlTile {
                Layout.fillWidth: true
                Layout.preferredWidth: 1
                visible: sidebar.settings.hotspotSupported === true
                enabled: sidebar.settings.hotspotBusy !== true
                iconName: "wifi"
                title: L10n.t("Hotspot")
                subtitle: sidebar.settings.hotspotBusy === true ? L10n.t("Connecting...")
                        : sidebar.settings.hotspotJoined === true ? L10n.t("Connected") : ""
                on: sidebar.settings.hotspotJoined === true
                onClicked: EngineBridge.perform(sidebar.settings.hotspotJoined === true ? "hotspot.disconnect" : "hotspot.connect", "")
            }

            ControlTile {
                readonly property var camera: { EngineBridge.settingsRevision; return JSON.parse(EngineBridge.query("camera", "")) }
                Layout.fillWidth: true
                Layout.preferredWidth: 1
                visible: camera !== null && camera.enabled === true && camera.registered === true
                iconName: "device"
                title: L10n.t("Camera")
                subtitle: camera !== null && camera.inUse === true ? L10n.t(camera.state === "Streaming" ? "Streaming" : "Connecting...") : ""
                on: camera !== null && camera.inUse === true
                onClicked: EngineBridge.perform("camera.use", String(!on))
            }

            ControlTile {
                Layout.fillWidth: true
                Layout.preferredWidth: 1
                visible: sidebar.settings.canSendFiles === true
                iconName: "upload"
                title: L10n.t("Send Files")
                onClicked: sendDialog.open()
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
